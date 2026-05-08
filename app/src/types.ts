export interface TelegramFile {
    id: number;
    name: string;
    size: number;
    sizeStr: string; // Formatted size
    created_at?: string;
    type?: 'folder' | 'file'; // implied icon_type
    // Add other fields if backend sends them
}

export interface TelegramFolder {
    id: number;
    name: string;
    parent_id?: number;
}

export interface QueueItem {
    id: string;
    path: string;
    folderId: number | null;
    status: 'pending' | 'uploading' | 'splitting' | 'success' | 'error' | 'cancelled';
    error?: string;
    /** Overall progress 0-100 across all parts */
    progress?: number;
    uploaded_bytes?: number;
    total_bytes?: number;
    speed_bytes_per_sec?: number;
    /** For split uploads: which part (1-based) */
    partIndex?: number;
    /** For split uploads: total number of parts */
    totalParts?: number;
    /** Parent queue item ID that spawned this part */
    parentTransferId?: string;
    /** Current part's individual progress 0-100 */
    partProgress?: number;
    /** Current part's uploaded bytes */
    partUploadedBytes?: number;
    /** Current part's total bytes */
    partTotalBytes?: number;
    /** Current part's speed */
    partSpeedBytesPerSec?: number;
}

export interface BandwidthStats {
    up_bytes: number;
    down_bytes: number;
}

export interface DownloadItem {
    id: string;
    messageId: number;
    filename: string;
    folderId: number | null;
    status: 'pending' | 'downloading' | 'success' | 'error' | 'cancelled';
    error?: string;
    progress?: number; // 0-100
    downloaded_bytes?: number;
    total_bytes?: number;
    speed_bytes_per_sec?: number;
}

export interface SplitItem {
    id: string;
    filename: string;
    status: 'splitting' | 'zipping' | 'partitioning' | 'success' | 'error';
    progress?: number; // 0-100
    message?: string;
}
