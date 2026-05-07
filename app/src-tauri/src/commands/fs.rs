use tauri::{State, Emitter};
use grammers_client::types::{Media, Peer};
use grammers_client::InputMessage;
use grammers_tl_types as tl;
use crate::TelegramState;
use crate::models::{FolderMetadata, FileMetadata};
use crate::bandwidth::BandwidthManager;
use crate::commands::utils::{resolve_peer, map_error};

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

                    let _ = self.app_handle.emit("upload-progress", ProgressPayload { 
                        id: self.transfer_id.clone(), 
                        percent,
                        uploaded_bytes: self.uploaded,
                        total_bytes: self.total_size,
                        speed_bytes_per_sec: speed,
                    });
                    self.last_emit = now;
                }
            }
        }
        poll_result
    }
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
    let file_metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    let size = file_metadata.len();
    bw_state.can_transfer(size)?;

    let tid = transfer_id.unwrap_or_default();

    let client_opt = { state.client.lock().await.clone() };
    if client_opt.is_none() {
        log::info!("[MOCK] Uploaded file {} to {:?}", path, folder_id);
        bw_state.add_up(size);
        return Ok("Mock upload successful".to_string());
    }
    let client = client_opt.unwrap();
    
    let file = tokio::fs::File::open(&path).await.map_err(|e| e.to_string())?;
    let mut progress_reader = ProgressReader {
        inner: file,
        total_size: size,
        uploaded: 0,
        start_time: Instant::now(),
        last_emit: Instant::now(),
        app_handle: app_handle.clone(),
        transfer_id: tid.clone(),
    };

    let file_name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    let client_clone = client.clone();

    let uploaded_file = tauri::async_runtime::spawn(async move {
        client_clone.upload_stream(&mut progress_reader, size as usize, file_name).await
    }).await.map_err(|e| format!("Task join error: {}", e))?
      .map_err(|e| format!("Upload error: {}", e))?;
        
    let message = InputMessage::new().text("").file(uploaded_file);
    let peer = resolve_peer(&client, folder_id, &state.peer_cache).await?;
    client.send_message(&peer, message).await.map_err(map_error)?;
    
    bw_state.add_up(size);

    if !tid.is_empty() {
        let _ = app_handle.emit("upload-progress", ProgressPayload { 
            id: tid, 
            percent: 100,
            uploaded_bytes: size,
            total_bytes: size,
            speed_bytes_per_sec: 0.0,
        });
    }

    Ok("File uploaded successfully".to_string())
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
