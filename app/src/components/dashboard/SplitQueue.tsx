import { useState } from 'react';
import { SplitItem } from "../../types";
import { Scissors, Minus, X, Check, Loader2 } from "lucide-react";

interface SplitQueueProps {
    items: SplitItem[];
    onRemove: (id: string) => void;
}

export function SplitQueue({ items, onRemove }: SplitQueueProps) {
    const [minimized, setMinimized] = useState(false);

    if (items.length === 0) return null;

    return (
        <div className="w-80 bg-telegram-surface border border-telegram-border rounded-xl shadow-2xl overflow-hidden z-[102]">
            <div className="p-3 border-b border-telegram-border bg-telegram-hover flex justify-between items-center">
                <div className="flex items-center gap-2">
                    <Scissors className="w-4 h-4 text-orange-400" />
                    <h4 className="text-sm font-medium text-telegram-text">Splitting</h4>
                    <span className="text-xs px-1.5 py-0.5 bg-orange-500/20 text-orange-400 rounded-full">
                        {items.length}
                    </span>
                </div>
                <div className="flex gap-1">
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
                                    {item.status === 'success' ? (
                                        <Check className="w-4 h-4 text-green-500" />
                                    ) : (
                                        <Loader2 className="w-4 h-4 text-orange-400 animate-spin" />
                                    )}
                                </div>
                                <div className="flex-1 truncate text-telegram-subtext" title={item.filename}>
                                    {item.filename}
                                </div>
                                {item.status === 'success' && (
                                    <button
                                        onClick={() => onRemove(item.id)}
                                        className="p-0.5 hover:bg-telegram-border rounded transition-colors"
                                    >
                                        <X className="w-3 h-3 text-telegram-subtext" />
                                    </button>
                                )}
                            </div>
                            {item.status !== 'success' && (
                                <div className="flex flex-col gap-1 w-full mt-1">
                                    <div className="flex justify-between items-center text-[10px] font-mono text-orange-400">
                                        <span className="capitalize">{item.status}</span>
                                        <span>{item.progress}% — {item.message}</span>
                                    </div>
                                    <div className="w-full bg-telegram-border h-1.5 rounded-full overflow-hidden">
                                        <div
                                            className="bg-orange-500 h-full rounded-full transition-all duration-300"
                                            style={{ width: `${item.progress || 0}%` }}
                                        />
                                    </div>
                                </div>
                            )}
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
}
