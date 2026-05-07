import { QueueItem } from "../../types";

interface UploadQueueProps {
    items: QueueItem[];
    onClearFinished: () => void;
    onCancelAll: () => void;
}

export function UploadQueue({ items, onClearFinished, onCancelAll }: UploadQueueProps) {
    if (items.length === 0) return null;

    const hasPendingOrActive = items.some(i => i.status === 'pending' || i.status === 'uploading');

    return (
        <div className="fixed bottom-4 right-4 w-80 bg-telegram-surface border border-telegram-border rounded-xl shadow-2xl overflow-hidden z-[100]">
            <div className="p-3 border-b border-telegram-border bg-telegram-hover flex justify-between items-center">
                <h4 className="text-sm font-medium text-telegram-text">Uploads</h4>
                <div className="flex gap-2">
                    {hasPendingOrActive && (
                        <button onClick={onCancelAll} className="text-xs text-red-400 hover:text-red-300 transition-colors">Cancel All</button>
                    )}
                    <button onClick={onClearFinished} className="text-xs text-telegram-primary hover:text-telegram-text transition-colors">Clear Finished</button>
                </div>
            </div>
            <div className="max-h-60 overflow-y-auto p-2 space-y-2">
                {items.map(item => (
                    <div key={item.id} className="flex flex-col gap-1 p-2 bg-telegram-hover rounded">
                        <div className="flex items-center gap-3 text-sm">
                            <div className={`w-2 h-2 rounded-full ${item.status === 'pending' ? 'bg-yellow-500' :
                                item.status === 'uploading' ? 'bg-blue-500 animate-pulse' :
                                    item.status === 'cancelled' ? 'bg-gray-500' :
                                        item.status === 'error' ? 'bg-red-500' : 'bg-green-500'
                                }`} />
                            <div className="flex-1 truncate text-telegram-subtext" title={item.path}>
                                {item.path.split('/').pop()}
                            </div>
                            {item.status === 'uploading' && item.progress !== undefined && (
                                <div className="text-xs text-blue-400 font-mono">{item.progress}%</div>
                            )}
                            {item.status === 'error' && <div className="text-xs text-red-400">Error</div>}
                            {item.status === 'cancelled' && <div className="text-xs text-gray-400">Cancelled</div>}
                        </div>
                        {item.status === 'uploading' && (
                            <div className="flex flex-col gap-1 w-full mt-1">
                                <div className="flex justify-between items-center text-[10px] font-mono text-blue-400">
                                    <span>
                                        {item.uploaded_bytes 
                                            ? `${(item.uploaded_bytes / 1024 / 1024).toFixed(2)} MB / ${(item.total_bytes! / 1024 / 1024).toFixed(2)} MB` 
                                            : 'Uploading...'}
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
                                            className="bg-blue-500 h-full rounded-full transition-all duration-300"
                                            style={{ width: `${item.progress}%` }}
                                        />
                                    ) : (
                                        <div className="bg-blue-500 h-full w-full animate-progress-indeterminate" />
                                    )}
                                </div>
                                <div className="text-right text-[9px] text-blue-400/70">
                                    {item.progress}%
                                </div>
                            </div>
                        )}
                    </div>
                ))}
            </div>
        </div>
    )
}
