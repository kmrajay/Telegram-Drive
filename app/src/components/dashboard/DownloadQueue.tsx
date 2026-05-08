import { useState } from 'react';
import { DownloadItem } from "../../types";
import { Download, Check, X, AlertCircle, Minus, Loader2 } from "lucide-react";

interface DownloadQueueProps {
    items: DownloadItem[];
    onClearFinished: () => void;
    onCancelAll: () => void;
    onCancelItem: (id: string) => void;
}

export function DownloadQueue({ items, onClearFinished, onCancelAll, onCancelItem }: DownloadQueueProps) {
    const [minimized, setMinimized] = useState(false);

    if (items.length === 0) return null;

    const activeCount = items.filter(i => i.status === 'pending' || i.status === 'downloading').length;
    const completedCount = items.filter(i => i.status === 'success').length;

    return (
        <div className="w-80 bg-telegram-surface border border-telegram-border rounded-xl shadow-2xl overflow-hidden z-[101]">
            <div className="p-3 border-b border-telegram-border bg-telegram-hover flex justify-between items-center">
                <div className="flex items-center gap-2">
                    <Download className="w-4 h-4 text-telegram-secondary" />
                    <h4 className="text-sm font-medium text-telegram-text">Downloads</h4>
                    {activeCount > 0 && (
                        <span className="text-xs px-1.5 py-0.5 bg-telegram-secondary/20 text-telegram-secondary rounded-full">
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
                    {activeCount > 0 && (
                        <button onClick={onCancelAll} className="text-xs text-red-400 hover:text-red-300 transition-colors mr-1">Cancel All</button>
                    )}
                    {completedCount > 0 && (
                        <button onClick={onClearFinished} className="text-xs text-telegram-primary hover:text-telegram-text transition-colors mr-1">
                            Clear
                        </button>
                    )}
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
                    {items.map(item => (
                        <div key={item.id} className="flex flex-col gap-1 p-2 bg-telegram-hover rounded">
                            <div className="flex items-center gap-3 text-sm">
                                <div className="flex-shrink-0">
                                    {item.status === 'pending' && <div className="w-4 h-4 rounded-full bg-yellow-500/20 flex items-center justify-center"><div className="w-2 h-2 bg-yellow-500 rounded-full" /></div>}
                                    {item.status === 'downloading' && <Loader2 className="w-4 h-4 text-telegram-secondary animate-spin" />}
                                    {item.status === 'success' && <Check className="w-4 h-4 text-green-500" />}
                                    {item.status === 'error' && <AlertCircle className="w-4 h-4 text-red-500" />}
                                    {item.status === 'cancelled' && <X className="w-4 h-4 text-gray-400" />}
                                </div>
                                <div className="flex-1 truncate text-telegram-subtext" title={item.filename}>
                                    {item.filename}
                                </div>
                                {/* Individual cancel button */}
                                {(item.status === 'pending' || item.status === 'downloading') && (
                                    <button
                                        onClick={() => onCancelItem(item.id)}
                                        className="p-0.5 hover:bg-red-500/20 rounded transition-colors group"
                                        title="Cancel"
                                    >
                                        <X className="w-3 h-3 text-telegram-subtext group-hover:text-red-400" />
                                    </button>
                                )}
                                {item.status === 'downloading' && item.progress !== undefined && (
                                    <div className="text-xs text-telegram-secondary font-mono">{item.progress}%</div>
                                )}
                                {item.status === 'cancelled' && <div className="text-xs text-gray-400">Cancelled</div>}
                            </div>
                            {item.status === 'downloading' && (
                                <div className="flex flex-col gap-1 w-full mt-1">
                                    <div className="flex justify-between items-center text-[10px] font-mono text-telegram-secondary">
                                        <span>
                                            {item.downloaded_bytes
                                                ? `${(item.downloaded_bytes / 1024 / 1024).toFixed(2)} MB / ${(item.total_bytes! / 1024 / 1024).toFixed(2)} MB`
                                                : 'Downloading...'}
                                        </span>
                                        <span>
                                            {item.speed_bytes_per_sec
                                                ? `${(item.speed_bytes_per_sec / 1024 / 1024).toFixed(2)} MB/s`
                                                : '0 MB/s'}
                                        </span>
                                    </div>
                                    <div className="w-full bg-telegram-border h-1.5 rounded-full overflow-hidden">
                                        {item.progress !== undefined ? (
                                            <div
                                                className="bg-telegram-secondary h-full rounded-full transition-all duration-300"
                                                style={{ width: `${item.progress}%` }}
                                            />
                                        ) : (
                                            <div className="bg-telegram-secondary h-full w-full animate-progress-indeterminate" />
                                        )}
                                    </div>
                                    <div className="text-right text-[9px] text-telegram-secondary/70">
                                        {item.progress}%
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
                    ))}
                </div>
            )}
            {minimized && (
                <div className="p-2 text-xs text-telegram-subtext text-center">
                    {activeCount > 0 ? `${activeCount} downloading...` : `${completedCount} completed`}
                </div>
            )}
        </div>
    );
}
