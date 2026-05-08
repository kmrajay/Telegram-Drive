use tauri::{State, Emitter};
use grammers_client::types::{Media, Peer, attributes::Attribute};
use grammers_client::InputMessage;
use grammers_tl_types as tl;
use crate::TelegramState;
use crate::models::{FolderMetadata, FileMetadata};
use crate::bandwidth::BandwidthManager;
use crate::commands::utils::{resolve_peer, map_error};
use std::process::Command;
use std::path::{Path, PathBuf};
use std::fs;
use std::io::Read;
use std::io::BufRead;

#[derive(Clone, serde::Serialize)]
pub struct SplitProgressPayload {
    pub id: String,
    pub filename: String,
    pub status: String, // "splitting", "zipping", "partitioning", "success"
    pub progress: u8,
    pub message: String,
}

#[derive(Clone, serde::Serialize)]
pub struct PartProgressPayload {
    pub parent_transfer_id: String,
    pub part_index: u64,
    pub total_parts: u64,
    pub percent: u8,
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_sec: f64,
}

#[tauri::command]
pub async fn cmd_create_folder(
    name: String,
    state: State<'_, TelegramState>,
) -> Result<FolderMetadata, String> {
    let client_opt = {
        state.client.lock().await.clone()
    };
    
    // --- MOCK ---
    if client_opt.is_none() {
        let mock_id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        log::info!("[MOCK] Created folder '{}' with ID {}", name, mock_id);
        return Ok(FolderMetadata {
            id: mock_id,
            name,
            parent_id: None,
        });
    }
    // -----------
    let client = client_opt.unwrap();
    log::info!("Creating Telegram Channel: {}", name);
    
    let result = client.invoke(&tl::functions::channels::CreateChannel {
        broadcast: true,
        megagroup: false,
        title: format!("{} [TD]", name),
        about: "Telegram Drive Storage Folder\n[telegram-drive-folder]".to_string(),
        geo_point: None,
        address: None,
        for_import: false,
        forum: false,
        ttl_period: None, // Initial creation TTL
    }).await.map_err(map_error)?;
    
    let (chat_id, access_hash) = match result {
        tl::enums::Updates::Updates(u) => {
             let chat = u.chats.first().ok_or("No chat in updates")?;
             match chat {
                 tl::enums::Chat::Channel(c) => (c.id, c.access_hash.unwrap_or(0)),
                 _ => return Err("Created chat is not a channel".to_string()),
             }
        },
        _ => return Err("Unexpected response (not Updates::Updates)".to_string()), 
    };

    // Explicitly Disable TTL
    let _input_channel = tl::enums::InputChannel::Channel(tl::types::InputChannel {
         channel_id: chat_id,
         access_hash,
    });

    let _ = client.invoke(&tl::functions::messages::SetHistoryTtl {
        peer: tl::enums::InputPeer::Channel(tl::types::InputPeerChannel { channel_id: chat_id, access_hash }),
        period: 0, 
    }).await;

    Ok(FolderMetadata {
        id: chat_id,
        name,
        parent_id: None,
    })
}

#[tauri::command]
pub async fn cmd_delete_folder(
    folder_id: i64,
    state: State<'_, TelegramState>,
) -> Result<bool, String> {
    let client_opt = {
        state.client.lock().await.clone()
    };
    
    if client_opt.is_none() {
        log::info!("[MOCK] Deleted folder ID {}", folder_id);
        return Ok(true);
    }
    let client = client_opt.unwrap();
    log::info!("Deleting folder/channel: {}", folder_id);

    let peer = resolve_peer(&client, Some(folder_id), &state.peer_cache).await?;
    
    let input_channel = match peer {
        Peer::Channel(c) => {
             let chan = &c.raw;
             tl::enums::InputChannel::Channel(tl::types::InputChannel {
                 channel_id: chan.id,
                 access_hash: chan.access_hash.ok_or("No access hash for channel")?,
             })
        },
        _ => return Err("Only channels (folders) can be deleted.".to_string()),
    };
    
    client.invoke(&tl::functions::channels::DeleteChannel {
        channel: input_channel,
    }).await.map_err(|e| format!("Failed to delete channel: {}", e))?;
    
    Ok(true)
}


#[derive(Clone, serde::Serialize)]
pub struct ProgressPayload {
    pub id: String,
    pub percent: u8,
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_bytes_per_sec: f64,
}

use tokio::io::{AsyncRead, ReadBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Instant, Duration};

struct ProgressReader<R> {
    inner: R,
    total_size: u64,
    uploaded: u64,
    start_time: Instant,
    last_emit: Instant,
    app_handle: tauri::AppHandle,
    transfer_id: String,
    /// If this is a part of a split upload, store parent info for part-progress events
    parent_transfer_id: Option<String>,
    part_index: Option<u64>,
    total_parts: Option<u64>,
}

impl<R: AsyncRead + Unpin> AsyncRead for ProgressReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let filled_before = buf.filled().len();
        let poll_result = Pin::new(&mut self.inner).poll_read(cx, buf);
        
        if let Poll::Ready(Ok(())) = poll_result {
            let filled_after = buf.filled().len();
            let bytes_read = (filled_after - filled_before) as u64;
            if bytes_read > 0 {
                self.uploaded += bytes_read;
                let now = Instant::now();
                
                if now.duration_since(self.last_emit) >= Duration::from_millis(250) {
                    let percent = (self.uploaded as f64 / self.total_size as f64 * 100.0).min(100.0) as u8;
                    let elapsed = now.duration_since(self.start_time).as_secs_f64();
                    let speed = if elapsed > 0.0 { self.uploaded as f64 / elapsed } else { 0.0 };

                    if let (Some(parent_id), Some(pidx), Some(tparts)) = (&self.parent_transfer_id, self.part_index, self.total_parts) {
                        // Emit part-specific progress event for split uploads
                        let _ = self.app_handle.emit("part-progress", PartProgressPayload {
                            parent_transfer_id: parent_id.clone(),
                            part_index: pidx,
                            total_parts: tparts,
                            percent,
                            uploaded_bytes: self.uploaded,
                            total_bytes: self.total_size,
                            speed_bytes_per_sec: speed,
                        });
                    } else {
                        // Regular single-file upload progress
                        let _ = self.app_handle.emit("upload-progress", ProgressPayload { 
                            id: self.transfer_id.clone(), 
                            percent,
                            uploaded_bytes: self.uploaded,
                            total_bytes: self.total_size,
                            speed_bytes_per_sec: speed,
                        });
                    }
                    self.last_emit = now;
                }
            }
        }
        poll_result
    }
}

// ============================================================
// Large File Splitting Logic
// ============================================================

/// 2 GB in bytes — Telegram's upload limit for regular accounts.
const TG_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
/// Safety buffer — target 1.8 GB to leave room for key-frame drift.
const SPLIT_TARGET: u64 = 1_932_735_283; // ~1.8 GB

/// Video metadata for proper Telegram media attributes
#[derive(Default)]
struct VideoMeta {
    duration_secs: f64,
    width: i32,
    height: i32,
}

/// Get video metadata (duration, width, height) using ffprobe.
fn get_video_meta(path: &Path) -> VideoMeta {
    let output = match Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height,duration",
            "-of", "csv=p=0",
            &path.to_string_lossy(),
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return VideoMeta::default(),
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    // Format: "1920,1080,3600.123" or "1920,1080,N/A"
    let parts: Vec<&str> = stdout.split(',').collect();
    if parts.len() >= 3 {
        VideoMeta {
            width: parts[0].trim().parse().unwrap_or(0),
            height: parts[1].trim().parse().unwrap_or(0),
            duration_secs: parts[2].trim().parse().unwrap_or(0.0),
        }
    } else {
        VideoMeta::default()
    }
}
/// Video file extensions that should be split into playable segments.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "3gp", "ts",
];

fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Split a large video into playable segments using FFmpeg.
/// Returns a list of segment file paths (all < TG_LIMIT).
/// Emits split-progress events for live UI updates.
fn split_video_ffmpeg(input: &Path, app_handle: &tauri::AppHandle, transfer_id: &str) -> Result<Vec<PathBuf>, String> {
    let temp_dir = input.parent().unwrap_or(Path::new("."))
        .join(format!(".td_split_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()));
    fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    // Get video duration in seconds using ffprobe
    let probe_output = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            &input.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("ffprobe failed (is ffmpeg installed?): {}", e))?;

    let duration_str = String::from_utf8_lossy(&probe_output.stdout).trim().to_string();
    let total_duration: f64 = duration_str.parse()
        .map_err(|e| format!("Could not parse video duration '{}': {}", duration_str, e))?;

    if total_duration <= 0.0 {
        return Err("Video duration is zero or negative".to_string());
    }

    let file_size = fs::metadata(input).map_err(|e| e.to_string())?.len();
    let num_segments = ((file_size as f64) / (SPLIT_TARGET as f64)).ceil() as u64;
    let segment_duration = total_duration / (num_segments as f64);

    log::info!(
        "Splitting video: {:.1}s total, {} bytes, target {} segments of {:.1}s each",
        total_duration, file_size, num_segments, segment_duration
    );

    // Emit initial progress
    let _ = app_handle.emit("split-progress", SplitProgressPayload {
        id: transfer_id.to_string(),
        filename: input.file_name().unwrap_or_default().to_string_lossy().to_string(),
        status: "splitting".to_string(),
        progress: 0,
        message: format!("Splitting into {} parts...", num_segments),
    });

    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let output_pattern = temp_dir.join(format!("part_%03d.{}", ext)).to_string_lossy().to_string();
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    let output_pattern = format!("{}.{}", output_pattern, ext);

    // Run FFmpeg with stderr piped so we can parse progress
    let mut ffmpeg_child = Command::new("ffmpeg")
        .args([
            "-i", &input.to_string_lossy(),
            "-c", "copy",
            "-map", "0",
            "-segment_time", &segment_duration.to_string(),
            "-f", "segment",
            "-reset_timestamps", "1",
            "-progress", "pipe:1",  // Output progress info to stdout
            &output_pattern,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ffmpeg failed (is ffmpeg installed?): {}", e))?;

    // Read stderr in a separate thread to parse progress
    let stderr = ffmpeg_child.stderr.take().unwrap();
    let stdout = ffmpeg_child.stdout.take().unwrap();
    let total_dur = total_duration;
    let num_seg = num_segments;
    let ah = app_handle.clone();
    let tid = transfer_id.to_string();
    let fname = input.file_name().unwrap_or_default().to_string_lossy().to_string();

    let progress_thread = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stderr);
        let mut last_percent: u8 = 0;
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    // FFmpeg outputs lines like:
                    //   out_time_ms=5000000  (microseconds)
                    //   frame=  120 fps= 60 ...
                    //   size=   15360kB time=00:00:05.00 ...
                    if let Some(time_str) = l.strip_prefix("out_time_ms=") {
                        if let Ok(us) = time_str.trim().parse::<f64>() {
                            let current_secs = us / 1_000_000.0;
                            let pct = ((current_secs / total_dur) * 100.0).min(99.0) as u8;
                            if pct > last_percent {
                                last_percent = pct;
                                // Calculate which segment we're on
                                let current_part = (current_secs / segment_duration).floor() as u64 + 1;
                                let part_display = current_part.min(num_seg);
                                let _ = ah.emit("split-progress", SplitProgressPayload {
                                    id: tid.clone(),
                                    filename: fname.clone(),
                                    status: "splitting".to_string(),
                                    progress: pct,
                                    message: format!("Splitting part {}/{}", part_display, num_seg),
                                });
                            }
                        }
                    } else if l.contains("time=") {
                        // Fallback: parse time=HH:MM:SS.ms from status line
                        if let Some(idx) = l.find("time=") {
                            let time_part = &l[idx + 5..];
                            let time_str = time_part.split_whitespace().next().unwrap_or("");
                            if let Some(secs) = parse_ffmpeg_time(time_str) {
                                let pct = ((secs / total_dur) * 100.0).min(99.0) as u8;
                                if pct > last_percent {
                                    last_percent = pct;
                                    let current_part = (secs / segment_duration).floor() as u64 + 1;
                                    let part_display = current_part.min(num_seg);
                                    let _ = ah.emit("split-progress", SplitProgressPayload {
                                        id: tid.clone(),
                                        filename: fname.clone(),
                                        status: "splitting".to_string(),
                                        progress: pct,
                                        message: format!("Splitting part {}/{}", part_display, num_seg),
                                    });
                                }
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Also drain stdout (progress output) to avoid pipe blocking
    let stdout_drain = std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stdout);
        for line in reader.lines() {
            if line.is_err() { break; }
        }
    });

    let status = ffmpeg_child.wait().map_err(|e| format!("ffmpeg wait error: {}", e))?;
    let _ = progress_thread.join();
    let _ = stdout_drain.join();

    if !status.success() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err("ffmpeg segment split failed".to_string());
    }

    // Collect the generated segments, sorted by name
    let mut parts: Vec<PathBuf> = fs::read_dir(&temp_dir)
        .map_err(|e| format!("Failed to read temp dir: {}", e))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    parts.sort();

    if parts.is_empty() {
        let _ = fs::remove_dir_all(&temp_dir);
        return Err("ffmpeg produced no output segments".to_string());
    }

    // Verify each segment is under TG_LIMIT
    for part in &parts {
        let sz = fs::metadata(part).map_err(|e| e.to_string())?.len();
        if sz > TG_LIMIT {
            log::warn!("Segment {} is {} bytes (over 2GB limit). May fail upload.", part.display(), sz);
        }
    }

    // Post-process: move moov atom to the start of each segment for streaming
    // This is a stream-copy operation (no re-encoding), so it's very fast
    for (i, part) in parts.iter().enumerate() {
        let faststart_path = temp_dir.join(format!(".faststart_{}", i));
        let faststart_result = Command::new("ffmpeg")
            .args([
                "-i", &part.to_string_lossy(),
                "-c", "copy",
                "-movflags", "+faststart",
                "-y",
                &faststart_path.to_string_lossy(),
            ])
            .output();
        
        match faststart_result {
            Ok(output) if output.status.success() => {
                // Replace original with faststart version
                if let Err(e) = fs::rename(&faststart_path, part) {
                    log::warn!("Failed to rename faststart file {}: {}", part.display(), e);
                    let _ = fs::remove_file(&faststart_path);
                }
            }
            Ok(output) => {
                log::warn!("faststart failed for {}: {}", part.display(), String::from_utf8_lossy(&output.stderr));
                let _ = fs::remove_file(&faststart_path);
            }
            Err(e) => {
                log::warn!("faststart ffmpeg failed for {}: {}", part.display(), e);
            }
        }
    }

    Ok(parts)
}

/// Parse FFmpeg time string like "00:01:23.45" into seconds.
fn parse_ffmpeg_time(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 { return None; }
    let hours: f64 = parts[0].parse().ok()?;
    let minutes: f64 = parts[1].parse().ok()?;
    let seconds: f64 = parts[2].parse().ok()?;
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

/// Zip and partition a non-video file into chunks < TG_LIMIT.
/// Uses 7z if available, falls back to Rust zip crate.
/// Emits split-progress events for live UI updates.
fn split_non_video(input: &Path, app_handle: &tauri::AppHandle, transfer_id: &str) -> Result<Vec<PathBuf>, String> {
    let temp_dir = input.parent().unwrap_or(Path::new("."))
        .join(format!(".td_split_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()));
    fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    let file_name = input.file_name().and_then(|n| n.to_str()).unwrap_or("file");

    // Emit initial progress
    let _ = app_handle.emit("split-progress", SplitProgressPayload {
        id: transfer_id.to_string(),
        filename: file_name.to_string(),
        status: "zipping".to_string(),
        progress: 0,
        message: "Compressing file...".to_string(),
    });

    // Try 7z first (fast & reliable)
    let archive_path = temp_dir.join(format!("{}.zip", file_name));
    let seven_zip_result = Command::new("7z")
        .args([
            "a",
            "-tzip",
            &format!("-v{}m", (SPLIT_TARGET / (1024 * 1024)) as u64),
            &archive_path.to_string_lossy(),
            &input.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match seven_zip_result {
        Ok(status) if status.success() => {
            // 7z creates archive.zip.001, archive.zip.002, etc.
            let mut parts: Vec<PathBuf> = fs::read_dir(&temp_dir)
                .map_err(|e| format!("Failed to read temp dir: {}", e))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            parts.sort();

            if parts.is_empty() {
                let _ = fs::remove_dir_all(&temp_dir);
                return Err("7z produced no output files".to_string());
            }

            // Emit completion
            let _ = app_handle.emit("split-progress", SplitProgressPayload {
                id: transfer_id.to_string(),
                filename: file_name.to_string(),
                status: "partitioning".to_string(),
                progress: 100,
                message: format!("Split into {} parts", parts.len()),
            });

            return Ok(parts);
        },
        _ => {
            log::info!("7z not available, falling back to Rust zip+split");
        },
    }

    // Fallback: use Rust zip crate to create a single zip, then binary-split it
    let zip_path = temp_dir.join(format!("{}.zip", file_name));
    let file_size = fs::metadata(input).map_err(|e| e.to_string())?.len();
    {
        let zip_file = fs::File::create(&zip_path).map_err(|e| format!("Failed to create zip: {}", e))?;
        let mut zip_writer = zip::ZipWriter::new(zip_file);
        let mut src_file = fs::File::open(input).map_err(|e| format!("Failed to open source: {}", e))?;

        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored) // No compression for speed
            .large_file(true);

        zip_writer.start_file(file_name, options).map_err(|e| format!("Zip start_file error: {}", e))?;

        // Copy with progress tracking
        let mut buf = [0u8; 8192];
        let mut bytes_written: u64 = 0;
        let mut last_emit_percent: u8 = 0;
        loop {
            let n = std::io::Read::read(&mut src_file, &mut buf).map_err(|e| e.to_string())?;
            if n == 0 { break; }
            std::io::Write::write_all(&mut zip_writer, &buf[..n]).map_err(|e| e.to_string())?;
            bytes_written += n as u64;
            let pct = ((bytes_written as f64 / file_size as f64) * 100.0).min(99.0) as u8;
            if pct > last_emit_percent {
                last_emit_percent = pct;
                let _ = app_handle.emit("split-progress", SplitProgressPayload {
                    id: transfer_id.to_string(),
                    filename: file_name.to_string(),
                    status: "zipping".to_string(),
                    progress: pct,
                    message: format!("Zipping... {}%", pct),
                });
            }
        }
        zip_writer.finish().map_err(|e| format!("Zip finish error: {}", e))?;
    }

    // Binary-split the zip into TG_LIMIT-sized chunks
    let zip_size = fs::metadata(&zip_path).map_err(|e| e.to_string())?.len();
    if zip_size <= TG_LIMIT {
        let _ = app_handle.emit("split-progress", SplitProgressPayload {
            id: transfer_id.to_string(),
            filename: file_name.to_string(),
            status: "partitioning".to_string(),
            progress: 100,
            message: "Split into 1 part".to_string(),
        });
        return Ok(vec![zip_path]);
    }

    // Calculate number of parts
    let num_parts = ((zip_size as f64) / (SPLIT_TARGET as f64)).ceil() as u64;
    let _ = app_handle.emit("split-progress", SplitProgressPayload {
        id: transfer_id.to_string(),
        filename: file_name.to_string(),
        status: "partitioning".to_string(),
        progress: 0,
        message: format!("Partitioning into {} parts...", num_parts),
    });

    let mut parts = Vec::new();
    let mut src = fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut chunk_idx: u64 = 0;
    let mut remaining = zip_size;
    let mut total_partitioned: u64 = 0;
    let mut last_partition_pct: u8 = 0;

    while remaining > 0 {
        let chunk_size = remaining.min(SPLIT_TARGET);
        let chunk_path = temp_dir.join(format!("{}.zip.{:03}", file_name, chunk_idx + 1));
        let mut chunk_file = fs::File::create(&chunk_path).map_err(|e| e.to_string())?;
        std::io::copy(&mut std::io::Read::by_ref(&mut src).take(chunk_size), &mut chunk_file).map_err(|e| e.to_string())?;
        parts.push(chunk_path);
        remaining -= chunk_size;
        total_partitioned += chunk_size;
        chunk_idx += 1;

        // Emit partition progress
        let pct = ((total_partitioned as f64 / zip_size as f64) * 100.0).min(99.0) as u8;
        if pct > last_partition_pct {
            last_partition_pct = pct;
            let _ = app_handle.emit("split-progress", SplitProgressPayload {
                id: transfer_id.to_string(),
                filename: file_name.to_string(),
                status: "partitioning".to_string(),
                progress: pct,
                message: format!("Partitioning part {}/{}", chunk_idx, num_parts),
            });
        }
    }

    // Remove the full zip (we only need the split parts)
    let _ = fs::remove_file(&zip_path);

    let _ = app_handle.emit("split-progress", SplitProgressPayload {
        id: transfer_id.to_string(),
        filename: file_name.to_string(),
        status: "partitioning".to_string(),
        progress: 100,
        message: format!("Partitioned into {} parts", parts.len()),
    });

    Ok(parts)
}

/// Decides whether and how to split a file.
/// Returns (list of paths to upload, optional temp dir to clean up).
fn prepare_file_for_upload(path: &str, app_handle: &tauri::AppHandle, transfer_id: &str) -> Result<(Vec<PathBuf>, Option<PathBuf>), String> {
    let file_path = Path::new(path);
    let size = fs::metadata(file_path).map_err(|e| e.to_string())?.len();

    if size <= TG_LIMIT {
        // File fits within Telegram's limit — upload as-is
        log::info!("File {} ({} bytes) fits within 2GB limit, uploading directly", path, size);
        return Ok((vec![file_path.to_path_buf()], None));
    }

    log::info!("File {} ({} bytes) exceeds 2GB limit, splitting...", path, size);

    if is_video(file_path) {
        log::info!("Detected video file — splitting into playable segments with FFmpeg");
        let parts = split_video_ffmpeg(file_path, app_handle, transfer_id)?;
        let temp_dir = parts.first().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        Ok((parts, temp_dir))
    } else {
        log::info!("Detected non-video file — zipping and partitioning");
        let parts = split_non_video(file_path, app_handle, transfer_id)?;
        let temp_dir = parts.first().and_then(|p| p.parent()).map(|p| p.to_path_buf());
        Ok((parts, temp_dir))
    }
}

/// Async cleanup of temp dir with retries for Windows file locks.
async fn cleanup_temp_dir_async(dir: PathBuf) {
    log::info!("Attempting async cleanup of temp dir: {}", dir.display());
    for attempt in 1..=10 {
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {
                log::info!("Cleaned up temp dir: {}", dir.display());
                return;
            }
            Err(e) if attempt < 10 => {
                log::warn!("Async attempt {} failed to clean up temp dir {}: {}. Retrying in {}ms...", 
                    attempt, dir.display(), e, 300 * attempt);
                tokio::time::sleep(std::time::Duration::from_millis(300 * attempt as u64)).await;
            }
            Err(e) => {
                log::error!("Failed to clean up temp dir after {} async attempts: {} — {}", attempt, dir.display(), e);
            }
        }
    }
}

/// Check if a transfer has been cancelled by the user.
async fn is_cancelled(transfer_id: &str, state: &State<'_, TelegramState>) -> bool {
    if transfer_id.is_empty() { return false; }
    let cancelled = state.cancelled_transfers.read().await;
    cancelled.contains(transfer_id)
}

/// Mark a transfer as cancelled and remove it from the set.
async fn consume_cancel(transfer_id: &str, state: &State<'_, TelegramState>) {
    let mut cancelled = state.cancelled_transfers.write().await;
    cancelled.remove(transfer_id);
}

#[tauri::command]
pub async fn cmd_cancel_transfer(
    transfer_id: String,
    state: State<'_, TelegramState>,
) -> Result<bool, String> {
    log::info!("Cancelling transfer: {}", transfer_id);
    // 1. Set the cancel flag (for checks between parts)
    {
        let mut cancelled = state.cancelled_transfers.write().await;
        cancelled.insert(transfer_id.clone());
    }
    // 2. Abort the active handle immediately if it exists
    {
        let mut handles = state.active_handles.write().await;
        if let Some(handle) = handles.remove(&transfer_id) {
            handle.abort();
            log::info!("Aborted active handle for transfer: {}", transfer_id);
        }
    }
    // 3. Clean up temp dir if one was created for this transfer
    let temp_dir_to_clean = {
        let mut td = state.temp_dirs.write().await;
        td.remove(&transfer_id)
    };
    if let Some(dir) = temp_dir_to_clean {
        log::info!("Cleaning up temp dir for cancelled transfer: {}", dir.display());
        // Spawn a background task to clean up after the file handles are released
        let dir_clone = dir.clone();
        tokio::spawn(async move {
            // Give a moment for the aborted task to release file handles
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            cleanup_temp_dir_async(dir_clone).await;
        });
    }
    Ok(true)
}

#[tauri::command]
pub async fn cmd_upload_file(
    path: String,
    folder_id: Option<i64>,
    transfer_id: Option<String>,
    app_handle: tauri::AppHandle,
    state: State<'_, TelegramState>,
    bw_state: State<'_, BandwidthManager>,
) -> Result<String, String> {
    let original_size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    bw_state.can_transfer(original_size)?;

    let tid = transfer_id.unwrap_or_default();

    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() {
        log::info!("[MOCK] Uploaded file {} to {:?}", path, folder_id);
        bw_state.add_up(original_size);
        return Ok("Mock upload successful".to_string());
    }
    let client = client_opt.unwrap();

    // ========== SPLITTING LOGIC ==========
    let file_name = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    if is_cancelled(&tid, &state).await {
        consume_cancel(&tid, &state).await;
        return Err("Upload cancelled".to_string());
    }

    let (parts_to_upload, temp_dir) = if original_size > TG_LIMIT {
        let split_status = if is_video(Path::new(&path)) { "splitting" } else { "zipping" };
        let _ = app_handle.emit("split-progress", SplitProgressPayload {
            id: tid.clone(),
            filename: file_name.clone(),
            status: split_status.to_string(),
            progress: 0,
            message: format!("Preparing {}...", split_status),
        });
        let result = prepare_file_for_upload(&path, &app_handle, &tid)?;
        let _ = app_handle.emit("split-progress", SplitProgressPayload {
            id: tid.clone(),
            filename: file_name.clone(),
            status: "success".to_string(),
            progress: 100,
            message: format!("Split into {} parts", result.0.len()),
        });
        result
    } else {
        (vec![PathBuf::from(&path)], None)
    };

    // Register temp dir so cancel can clean it up
    if let Some(ref dir) = temp_dir {
        let mut td = state.temp_dirs.write().await;
        td.insert(tid.clone(), dir.clone());
    }

    let total_parts = parts_to_upload.len();
    log::info!("Uploading {} part(s) for file: {}", total_parts, path);

    if !tid.is_empty() {
        let _ = app_handle.emit("upload-progress", ProgressPayload {
            id: tid.clone(),
            percent: 0,
            uploaded_bytes: 0,
            total_bytes: original_size,
            speed_bytes_per_sec: 0.0,
        });
    }

    let mut total_uploaded: u64 = 0;
    let start_time = Instant::now();

    for (idx, part_path) in parts_to_upload.iter().enumerate() {
        // Check cancellation before each part
        if is_cancelled(&tid, &state).await {
            consume_cancel(&tid, &state).await;
            if let Some(ref dir) = temp_dir { cleanup_temp_dir_async(dir.clone()).await; }
            { let mut td = state.temp_dirs.write().await; td.remove(&tid); }
            return Err("Upload cancelled".to_string());
        }

        let part_size = fs::metadata(part_path).map_err(|e| e.to_string())?.len();
        let part_name = if total_parts > 1 {
            let base_name = Path::new(&path)
                .file_stem()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
            let ext = Path::new(&path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e))
                .unwrap_or_default();
            format!("{}.part{}of{}{}", base_name, idx + 1, total_parts, ext)
        } else {
            Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string())
        };

        log::info!("Uploading part {}/{}: {} ({} bytes)", idx + 1, total_parts, part_name, part_size);

        if !tid.is_empty() && total_parts > 1 {
            let _ = app_handle.emit("upload-part-status", serde_json::json!({
                "parentTransferId": tid,
                "partIndex": idx + 1,
                "totalParts": total_parts,
                "status": "uploading"
            }));
        }

        let file = tokio::fs::File::open(&part_path).await.map_err(|e| e.to_string())?;
        let part_transfer_id = if total_parts > 1 { format!("{}_part{}", tid, idx + 1) } else { tid.clone() };
        let mut progress_reader = ProgressReader {
            inner: file,
            total_size: part_size,
            uploaded: 0,
            start_time: Instant::now(),
            last_emit: Instant::now(),
            app_handle: app_handle.clone(),
            transfer_id: part_transfer_id.clone(),
            parent_transfer_id: if total_parts > 1 { Some(tid.clone()) } else { None },
            part_index: if total_parts > 1 { Some((idx + 1) as u64) } else { None },
            total_parts: if total_parts > 1 { Some(total_parts as u64) } else { None },
        };

        let client_clone = client.clone();
        let upload_name = part_name.clone();

        // Spawn the upload as a separate task and register the handle for abort
        let upload_handle =
            tokio::spawn(async move {
                client_clone.upload_stream(&mut progress_reader, part_size as usize, upload_name).await
            });

        // Register the handle so cmd_cancel_transfer can abort it
        {
            let mut handles = state.active_handles.write().await;
            handles.insert(tid.clone(), upload_handle.abort_handle());
        }

        // Await the upload
        let upload_result = upload_handle.await;

        // Remove from active handles
        {
            let mut handles = state.active_handles.write().await;
            handles.remove(&tid);
        }

        // Check if the task was aborted (cancelled)
        match upload_result {
            Err(join_error) if join_error.is_cancelled() => {
                log::info!("Upload task aborted for transfer: {}", tid);
                consume_cancel(&tid, &state).await;
                if let Some(ref dir) = temp_dir { cleanup_temp_dir_async(dir.clone()).await; }
                { let mut td = state.temp_dirs.write().await; td.remove(&tid); }
                return Err("Upload cancelled".to_string());
            }
            Err(join_error) => {
                if let Some(ref dir) = temp_dir { cleanup_temp_dir_async(dir.clone()).await; }
                { let mut td = state.temp_dirs.write().await; td.remove(&tid); }
                return Err(format!("Task join error: {}", join_error));
            }
            Ok(Err(e)) => {
                if let Some(ref dir) = temp_dir { cleanup_temp_dir_async(dir.clone()).await; }
                { let mut td = state.temp_dirs.write().await; td.remove(&tid); }
                return Err(format!("Upload error on part {}/{}: {}", idx + 1, total_parts, e));
            }
            Ok(Ok(uploaded_file)) => {
                // Upload succeeded, continue
                let caption = if total_parts > 1 {
                    format!("Part {}/{}", idx + 1, total_parts)
                } else {
                    "".to_string()
                };

                // Build message based on file type:
                // - Videos: use .document() + Attribute::Video(supports_streaming) so Telegram processes them for streaming
                // - Non-videos: use .file() as before (force_file: true, no Telegram processing)
                let is_vid = is_video(part_path);
                let peer = resolve_peer(&client, folder_id, &state.peer_cache).await?;

                if is_vid {
                    let meta = get_video_meta(part_path);
                    let mut msg = InputMessage::new().text(&caption).document(uploaded_file);

                    // Add video attribute so Telegram knows it's streamable
                    if meta.duration_secs > 0.0 || meta.width > 0 {
                        msg = msg.attribute(Attribute::Video {
                            round_message: false,
                            supports_streaming: true,
                            duration: std::time::Duration::from_secs_f64(if meta.duration_secs > 0.0 { meta.duration_secs } else { 1.0 }),
                            w: if meta.width > 0 { meta.width } else { 1280 },
                            h: if meta.height > 0 { meta.height } else { 720 },
                        });
                    } else {
                        // No metadata — still mark as streamable video with defaults
                        msg = msg.attribute(Attribute::Video {
                            round_message: false,
                            supports_streaming: true,
                            duration: std::time::Duration::from_secs(1),
                            w: 1280,
                            h: 720,
                        });
                    }

                    client.send_message(&peer, msg).await.map_err(map_error)?;
                } else {
                    let message = InputMessage::new().text(&caption).file(uploaded_file);
                    client.send_message(&peer, message).await.map_err(map_error)?;
                }

                if !tid.is_empty() && total_parts > 1 {
                    let _ = app_handle.emit("upload-part-status", serde_json::json!({
                        "parentTransferId": tid,
                        "partIndex": idx + 1,
                        "totalParts": total_parts,
                        "status": "success"
                    }));
                }

                total_uploaded += part_size;
                bw_state.add_up(part_size);

                if !tid.is_empty() {
                    let overall_percent = ((total_uploaded as f64 / original_size as f64) * 100.0).min(100.0) as u8;
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 { total_uploaded as f64 / elapsed } else { 0.0 };
                    let _ = app_handle.emit("upload-progress", ProgressPayload {
                        id: tid.clone(),
                        percent: overall_percent,
                        uploaded_bytes: total_uploaded,
                        total_bytes: original_size,
                        speed_bytes_per_sec: speed,
                    });
                }
            }
        }
    }

    // Clean up temp files if we split
    if let Some(ref dir) = temp_dir {
        cleanup_temp_dir_async(dir.clone()).await;
        let mut td = state.temp_dirs.write().await;
        td.remove(&tid);
    }

    if !tid.is_empty() {
        let _ = app_handle.emit("upload-progress", ProgressPayload {
            id: tid,
            percent: 100,
            uploaded_bytes: original_size,
            total_bytes: original_size,
            speed_bytes_per_sec: 0.0,
        });
    }

    let result_msg = if total_parts > 1 {
        format!("File split into {} parts and uploaded successfully", total_parts)
    } else {
        "File uploaded successfully".to_string()
    };

    Ok(result_msg)
}

#[tauri::command]
pub async fn cmd_delete_file(
    message_id: i32,
    folder_id: Option<i64>,
    state: State<'_, TelegramState>,
) -> Result<bool, String> {
    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() { 
         log::info!("[MOCK] Deleted message {} from folder {:?}", message_id, folder_id);
        return Ok(true); 
    }
    let client = client_opt.unwrap();

    let peer = resolve_peer(&client, folder_id, &state.peer_cache).await?;
    client.delete_messages(&peer, &[message_id]).await.map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
pub async fn cmd_download_file(
    message_id: i32,
    save_path: String,
    folder_id: Option<i64>,
    transfer_id: Option<String>,
    app_handle: tauri::AppHandle,
    state: State<'_, TelegramState>,
    bw_state: State<'_, BandwidthManager>,
) -> Result<String, String> {
    let tid = transfer_id.unwrap_or_default();

    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() { 
        log::info!("[MOCK] Downloaded message {} from {:?} to {}", message_id, folder_id, save_path);
        if let Err(e) = std::fs::write(&save_path, b"Mock Content") { return Err(e.to_string()); }
        return Ok("Download successful".to_string());
    }
    let client = client_opt.unwrap();
    
    let peer = resolve_peer(&client, folder_id, &state.peer_cache).await?;

    // Use get_messages_by_id for efficient message lookup (same as server.rs)
    let messages = client.get_messages_by_id(&peer, &[message_id]).await.map_err(|e| e.to_string())?;
    
    let msg = messages.into_iter()
        .flatten()
        .next()
        .ok_or_else(|| "Message not found".to_string())?;

    let media = msg.media()
        .ok_or_else(|| "No media in message".to_string())?;

    let total_size = match &media {
        Media::Document(d) => d.size() as u64,
        Media::Photo(_) => 1024 * 1024,
        _ => 0,
    };
    
    bw_state.can_transfer(total_size)?;

    // Emit start
    if !tid.is_empty() {
        let _ = app_handle.emit("download-progress", ProgressPayload {
            id: tid.clone(),
            percent: 0,
            uploaded_bytes: 0,
            total_bytes: total_size,
            speed_bytes_per_sec: 0.0,
        });
    }

    // Stream download with per-chunk progress
    let mut download_iter = client.iter_download(&media);
    let mut file = std::fs::File::create(&save_path).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut last_percent: u8 = 0;
    let dl_start = Instant::now();
    let mut dl_last_emit = Instant::now();

    while let Some(chunk) = download_iter.next().await.transpose() {
        // Check cancellation during download
        if is_cancelled(&tid, &state).await {
            consume_cancel(&tid, &state).await;
            // Clean up partial download
            let _ = fs::remove_file(&save_path);
            return Err("Download cancelled".to_string());
        }

        let bytes = chunk.map_err(|e| format!("Download chunk error: {}", e))?;
        std::io::Write::write_all(&mut file, &bytes).map_err(|e| e.to_string())?;
        downloaded += bytes.len() as u64;
        
        if !tid.is_empty() && total_size > 0 {
            let now = Instant::now();
            let percent = ((downloaded as f64 / total_size as f64) * 100.0).min(100.0) as u8;
            // Emit when percent changes or every 250ms (whichever comes first)
            if percent != last_percent || now.duration_since(dl_last_emit) >= Duration::from_millis(250) {
                last_percent = percent;
                let elapsed = now.duration_since(dl_start).as_secs_f64();
                let speed = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };
                let _ = app_handle.emit("download-progress", ProgressPayload {
                    id: tid.clone(),
                    percent,
                    uploaded_bytes: downloaded,
                    total_bytes: total_size,
                    speed_bytes_per_sec: speed,
                });
                dl_last_emit = now;
            }
        }
    }

    bw_state.add_down(total_size);

    // Emit completion
    if !tid.is_empty() {
        let _ = app_handle.emit("download-progress", ProgressPayload {
            id: tid,
            percent: 100,
            uploaded_bytes: total_size,
            total_bytes: total_size,
            speed_bytes_per_sec: 0.0,
        });
    }

    Ok("Download successful".to_string())
}

#[tauri::command]
pub async fn cmd_move_files(
    message_ids: Vec<i32>,
    source_folder_id: Option<i64>,
    target_folder_id: Option<i64>,
    state: State<'_, TelegramState>,
) -> Result<bool, String> {
    if source_folder_id == target_folder_id { return Ok(true); }
    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() { 
        log::info!("[MOCK] Moved msgs {:?} from {:?} to {:?}", message_ids, source_folder_id, target_folder_id);
        return Ok(true); 
    }
    let client = client_opt.unwrap();

    let source_peer = resolve_peer(&client, source_folder_id, &state.peer_cache).await?;
    let target_peer = resolve_peer(&client, target_folder_id, &state.peer_cache).await?;

    match client.forward_messages(&target_peer, &message_ids, &source_peer).await {
        Ok(_) => {},
        Err(e) => return Err(format!("Forward failed: {}", e)),
    }
    
    match client.delete_messages(&source_peer, &message_ids).await {
        Ok(_) => {},
        Err(e) => return Err(format!("Delete original failed: {}", e)),
    }

    Ok(true)
}

#[tauri::command]
pub async fn cmd_get_files(
    folder_id: Option<i64>,
    state: State<'_, TelegramState>,
) -> Result<Vec<FileMetadata>, String> {
    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() { 
        log::info!("[MOCK] Returning mock files for folder {:?}", folder_id);
        return Ok(Vec::new()); // No mock files for now
    }
    let client = client_opt.unwrap();
    let mut files = Vec::new();
    
    let peer = resolve_peer(&client, folder_id, &state.peer_cache).await?;

    let mut msgs = client.iter_messages(&peer);
    while let Some(msg) = msgs.next().await.map_err(|e| e.to_string())? {
        if let Some(doc) = msg.media() {
            let (name, size, mime, ext) = match doc {
                Media::Document(d) => {
                    let n = d.name().to_string();
                    let s = d.size();
                    let m = d.mime_type().map(|s| s.to_string());
                    let e = std::path::Path::new(&n).extension().map(|os| os.to_str().unwrap_or("").to_string());
                    (n, s, m, e)
                },
                Media::Photo(_) => ("Photo.jpg".to_string(), 0, Some("image/jpeg".into()), Some("jpg".into())),
                _ => ("Unknown".to_string(), 0, None, None),
            };
            files.push(FileMetadata {
                id: msg.id() as i64, folder_id, name, size: size as u64, mime_type: mime, file_ext: ext, created_at: msg.date().to_string(), icon_type: "file".into()
            });
        }
    }

    Ok(files)
}

#[tauri::command]
pub async fn cmd_search_global(
    query: String,
    state: State<'_, TelegramState>,
) -> Result<Vec<FileMetadata>, String> {
    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() { 
        return Ok(Vec::new());
    }
    let client = client_opt.unwrap();
    let mut files = Vec::new();
    
    log::info!("Searching global for: {}", query);

    let result = client.invoke(&tl::functions::messages::SearchGlobal {
        q: query,
        filter: tl::enums::MessagesFilter::InputMessagesFilterDocument,
        min_date: 0,
        max_date: 0,
        offset_rate: 0,
        offset_peer: tl::enums::InputPeer::Empty,
        offset_id: 0,
        limit: 50,
        folder_id: None,
        broadcasts_only: false,
        groups_only: false,
        users_only: false,
    }).await.map_err(map_error)?;

    if let tl::enums::messages::Messages::Messages(msgs) = result {
        for msg in msgs.messages {
            if let tl::enums::Message::Message(m) = msg {
                if let Some(tl::enums::MessageMedia::Document(d)) = m.media {
                    if let tl::enums::Document::Document(doc) = d.document.unwrap() {
                        let name = doc.attributes.iter().find_map(|a| match a {
                            tl::enums::DocumentAttribute::Filename(f) => Some(f.file_name.clone()),
                            _ => None
                        }).unwrap_or("Unknown".to_string());
                        let size = doc.size as u64;
                        let mime = doc.mime_type.clone();
                        let ext = std::path::Path::new(&name).extension().map(|os| os.to_str().unwrap_or("").to_string());
                        let folder_id = match m.peer_id {
                            tl::enums::Peer::Channel(c) => Some(c.channel_id),
                            tl::enums::Peer::User(u) => Some(u.user_id),
                            tl::enums::Peer::Chat(c) => Some(c.chat_id),
                        };
                        files.push(FileMetadata {
                            id: m.id as i64, folder_id, name, size,
                            mime_type: Some(mime), file_ext: ext,
                            created_at: m.date.to_string(), icon_type: "file".into()
                        });
                    }
                }
            }
        }
    } else if let tl::enums::messages::Messages::Slice(msgs) = result {
        for msg in msgs.messages {
            if let tl::enums::Message::Message(m) = msg {
                if let Some(tl::enums::MessageMedia::Document(d)) = m.media {
                    if let tl::enums::Document::Document(doc) = d.document.unwrap() {
                        let name = doc.attributes.iter().find_map(|a| match a {
                            tl::enums::DocumentAttribute::Filename(f) => Some(f.file_name.clone()),
                            _ => None
                        }).unwrap_or("Unknown".to_string());
                        let size = doc.size as u64;
                        let mime = doc.mime_type.clone();
                        let ext = std::path::Path::new(&name).extension().map(|os| os.to_str().unwrap_or("").to_string());
                        let folder_id = match m.peer_id {
                            tl::enums::Peer::Channel(c) => Some(c.channel_id),
                            tl::enums::Peer::User(u) => Some(u.user_id),
                            tl::enums::Peer::Chat(c) => Some(c.chat_id),
                        };
                        files.push(FileMetadata {
                            id: m.id as i64, folder_id, name, size,
                            mime_type: Some(mime), file_ext: ext,
                            created_at: m.date.to_string(), icon_type: "file".into()
                        });
                    }
                }
            }
        }
    }

    Ok(files)
}

#[tauri::command]
pub async fn cmd_scan_folders(
    state: State<'_, TelegramState>,
) -> Result<Vec<FolderMetadata>, String> {
    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() { 
        return Ok(Vec::new());
    }
    let client = client_opt.unwrap();
    
    let mut folders = Vec::new();
    let mut dialogs = client.iter_dialogs();
    
    log::info!("Starting Folder Scan...");

    // Acquire write lock once for the entire scan to populate the peer cache
    let mut peer_cache = state.peer_cache.write().await;

    while let Some(dialog) = dialogs.next().await.map_err(|e| e.to_string())? {
        // Populate peer cache for every dialog we encounter (free priming)
        match &dialog.peer {
            Peer::Channel(c) => {
                let id = c.raw.id;
                peer_cache.insert(id, dialog.peer.clone());

                let name = c.raw.title.clone();
                let access_hash = c.raw.access_hash.unwrap_or(0);
                
                log::debug!("[SCAN] Processing Channel: '{}' (ID: {})", name, id);

                // Strategy 1: Title
                if name.to_lowercase().contains("[td]") {
                    log::info!(" -> MATCH via Title: {}", name);
                    let display_name = name.replace(" [TD]", "").replace(" [td]", "").replace("[TD]", "").replace("[td]", "").trim().to_string();
                    folders.push(FolderMetadata { id, name: display_name, parent_id: None });
                    continue; 
                }

                // Strategy 2: About
                let input_chan = tl::enums::InputChannel::Channel(tl::types::InputChannel {
                    channel_id: c.raw.id,
                    access_hash,
                });
                
                match client.invoke(&tl::functions::channels::GetFullChannel {
                    channel: input_chan,
                }).await {
                    Ok(tl::enums::messages::ChatFull::Full(f)) => {
                        if let tl::enums::ChatFull::Full(cf) = f.full_chat {
                             if cf.about.contains("[telegram-drive-folder]") {
                                 log::info!(" -> MATCH via About: {}", name);
                                 folders.push(FolderMetadata { id, name: name.clone(), parent_id: None });
                             }
                        }
                    },
                    Err(e) => log::warn!(" -> Failed to get full info: {}", e),
                }
            },
            Peer::User(u) => {
                peer_cache.insert(u.raw.id(), dialog.peer.clone());
                log::debug!("[SCAN] Cached User Peer: {}", u.raw.id());
            },
            peer => {
                log::debug!("[SCAN] Skipped Peer: {:?}", peer);
            }
        }
    }
    
    log::info!("Scan complete. Found {} folders. Peer cache size: {}.", folders.len(), peer_cache.len());
    Ok(folders)
}
