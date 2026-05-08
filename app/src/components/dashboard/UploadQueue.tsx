import { useState } from 'react';
import { QueueItem } from "../../types";
import { Upload, Minus, X, Loader2, Check, AlertCircle, Scissors } from "lucide-react";

interface UploadQueueProps {
    items: QueueItem[];
    onClearFinished: () => void;
    onCancelAll: () => void;
    onCancelItem: (id: string) => void;
}

function formatBytes(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(2)} MB`;
    return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatSpeed(bytesPerSec: number): string {
    if (bytesPerSec < 1024 * 1024) return `${(bytesPerSec / 1024).toFixed(1)} KB/s`;
    return `${(bytesPerSec / 1024 / 1024).toFixed(2)} MB/s`;
}

export function UploadQueue({ items, onClearFinished, onCancelAll, onCancelItem }: UploadQueueProps) {
    const [minimized, setMinimized] = useState(false);

    if (items.length === 0) return null;

    const hasPendingOrActive = items.some(i => i.status === 'pending' || i.status === 'uploading' || i.status === 'splitting');
    const activeCount = items.filter(i => i.status === 'uploading' || i.status === 'splitting').length;
    const completedCount = items.filter(i => i.status === 'success').length;

    return (
        <div className="w-80 bg-telegram-surface border border-telegram-border rounded-xl shadow-2xl overflow-hidden z-[100]">
            <div className="p-3 border-b border-telegram-border bg-telegram-hover flex justify-between items-center">
                <div className="flex items-center gap-2">
                    <Upload className="w-4 h-4 text-blue-400" />
                    <h4 className="text-sm font-medium text-telegram-text">Uploads</h4>
                    {activeCount > 0 && (
                        <span className="text-xs px-1.5 py-0.5 bg-blue-500/20 text-blue-400 rounded-full">
                            {activeCount} active
                        </span>
                    )}
                    {completedCount > 0 && (
                        <span className="text-xs px-1.5 py-0.5 bg-green-500/20 text-green-400 rounded-full">
                            {completedCount} done
                        </span>
                    )}
                </div>
                <div className="flex items-center gap-1">
                    {hasPendingOrActive && (
                        <button onClick={onCancelAll} className="text-xs text-red-400 hover:text-red-300 transition-colors mr-1">Cancel All</button>
                    )}
                    <button onClick={onClearFinished} className="text-xs text-telegram-primary hover:text-telegram-text transition-colors mr-1">Clear</button>
                    <button
                        onClick={() => setMinimized(!minimized)}
                        className="p-1 hover:bg-telegram-border rounded transition-colors"
                        title={minimized ? "Expand" : "Minimize"}
                    >
                        <Minus className="w-3 h-3 text-telegram-subtext" />
                    </button>
                </div>
            </div>
            {!minimized && (
                <div className="max-h-60 overflow-y-auto p-2 space-y-2">
                    {items.map(item => {
                        const isSplitFile = item.totalParts && item.totalParts > 1;
                        return (
                            <div key={item.id} className="flex flex-col gap-1 p-2 bg-telegram-hover rounded">
                                <div className="flex items-center gap-3 text-sm">
                                    <div className="flex-shrink-0">
                                        {item.status === 'pending' && <div className="w-4 h-4 rounded-full bg-yellow-500/20 flex items-center justify-center"><div className="w-2 h-2 bg-yellow-500 rounded-full" /></div>}
                                        {item.status === 'splitting' && <Scissors className="w-4 h-4 text-orange-400 animate-pulse" />}
                                        {item.status === 'uploading' && <Loader2 className="w-4 h-4 text-blue-400 animate-spin" />}
                                        {item.status === 'success' && <Check className="w-4 h-4 text-green-500" />}
                                        {item.status === 'error' && <AlertCircle className="w-4 h-4 text-red-500" />}
                                        {item.status === 'cancelled' && <X className="w-4 h-4 text-gray-400" />}
                                    </div>
                                    <div className="flex-1 truncate text-telegram-subtext" title={item.path}>
                                        {item.path.split(/[\\/]/).pop()}
                                    </div>
                                    {(item.status === 'pending' || item.status === 'uploading' || item.status === 'splitting') && (
                                        <button
                                            onClick={() => onCancelItem(item.id)}
                                            className="p-0.5 hover:bg-red-500/20 rounded transition-colors group"
                                            title="Cancel"
                                        >
                                            <X className="w-3 h-3 text-telegram-subtext group-hover:text-red-400" />
                                        </button>
                                    )}
                                    {item.status === 'uploading' && !isSplitFile && item.progress !== undefined && (
                                        <div className="text-xs text-blue-400 font-mono">{item.progress}%</div>
                                    )}
                                    {item.status === 'error' && <div className="text-xs text-red-400">Error</div>}
                                    {item.status === 'cancelled' && <div className="text-xs text-gray-400">Cancelled</div>}
                                </div>

                                {/* Splitting status */}
                                {item.status === 'splitting' && (
                                    <div className="text-[10px] font-mono text-orange-400 ml-7">
                                        Splitting into {item.totalParts} parts...
                                    </div>
                                )}

                                {/* Normal (non-split) upload progress */}
                                {item.status === 'uploading' && !isSplitFile && (
                                    <div className="flex flex-col gap-1 w-full mt-1">
                                        <div className="flex justify-between items-center text-[10px] font-mono text-blue-400">
                                            <span>
                                                {item.uploaded_bytes
                                                    ? `${formatBytes(item.uploaded_bytes)} / ${formatBytes(item.total_bytes!)}`
                                                    : 'Uploading...'}
                                            </span>
                                            <span>
                                                {item.speed_bytes_per_sec ? formatSpeed(item.speed_bytes_per_sec) : '0 MB/s'}
                                            </span>
                                        </div>
                                        <div className="w-full bg-telegram-border h-1.5 rounded-full overflow-hidden">
                                            <div
                                                className="bg-blue-500 h-full rounded-full transition-all duration-300"
                                                style={{ width: `${item.progress || 0}%` }}
                                            />
                                        </div>
                                    </div>
                                )}

                                {/* Split file upload: dual progress bars */}
                                {item.status === 'uploading' && isSplitFile && (
                                    <div className="flex flex-col gap-1.5 w-full mt-1">
                                        {/* Overall progress bar */}
                                        <div className="flex flex-col gap-0.5">
                                            <div className="flex justify-between items-center text-[10px] font-mono">
                                                <span className="text-telegram-subtext">
                                                    Overall — Part {item.partIndex || 1}/{item.totalParts}
                                                </span>
                                                <span className="text-blue-400">
                                                    {item.progress || 0}%
                                                </span>
                                            </div>
                                            <div className="w-full bg-telegram-border h-1 rounded-full overflow-hidden">
                                                <div
                                                    className="bg-blue-500/60 h-full rounded-full transition-all duration-300"
                                                    style={{ width: `${item.progress || 0}%` }}
                                                />
                                            </div>
                                        </div>

                                        {/* Current part progress bar */}
                                        <div className="flex flex-col gap-0.5">
                                            <div className="flex justify-between items-center text-[10px] font-mono">
                                                <span className="text-blue-300">
                                                    Part {item.partIndex || 1}
                                                </span>
                                                <span className="text-blue-400">
                                                    {item.partProgress !== undefined ? `${item.partProgress}%` : '0%'}
                                                </span>
                                            </div>
                                            <div className="w-full bg-telegram-border h-1.5 rounded-full overflow-hidden">
                                                <div
                                                    className="bg-blue-500 h-full rounded-full transition-all duration-300"
                                                    style={{ width: `${item.partProgress || 0}%` }}
                                                />
                                            </div>
                                            <div className="flex justify-between items-center text-[9px] font-mono text-blue-400/70">
                                                <span>
                                                    {item.partUploadedBytes && item.partTotalBytes
                                                        ? `${formatBytes(item.partUploadedBytes)} / ${formatBytes(item.partTotalBytes)}`
                                                        : ''}
                                                </span>
                                                <span>
                                                    {item.partSpeedBytesPerSec ? formatSpeed(item.partSpeedBytesPerSec) : ''}
                                                </span>
                                            </div>
                                        </div>
                                    </div>
                                )}

                                {item.status === 'error' && item.error && (
                                    <div className="flex items-center gap-1 text-xs text-red-400 mt-1">
                                        <AlertCircle className="w-3 h-3" />
                                        <span className="truncate">{item.error}</span>
                                    </div>
                                )}
                            </div>
                        );
                    })}
                </div>
            )}
            {minimized && (
                <div className="p-2 text-xs text-telegram-subtext text-center">
                    {activeCount > 0 ? `${activeCount} uploading...` : `${completedCount} completed`}
                </div>
            )}
        </div>
    );
}
