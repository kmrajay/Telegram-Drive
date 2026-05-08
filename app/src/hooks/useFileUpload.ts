import { useState, useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { listen, UnlistenFn } from '@tauri-apps/api/event';
import { useQueryClient } from '@tanstack/react-query';
import { toast } from 'sonner';
import { QueueItem, SplitItem } from '../types';
import { useFileDrop } from './useFileDrop';
import type { Store } from '@tauri-apps/plugin-store';

interface ProgressPayload {
    id: string;
    percent: number;
    uploaded_bytes: number;
    total_bytes: number;
    speed_bytes_per_sec: number;
}

interface SplitProgressPayload {
    id: string;
    filename: string;
    status: string;
    progress: number;
    message: string;
}

interface PartStatusPayload {
    parentTransferId: string;
    partIndex: number;
    totalParts: number;
    status: string;
}

export function useFileUpload(activeFolderId: number | null, store: Store | null) {
    const queryClient = useQueryClient();
    const [uploadQueue, setUploadQueue] = useState<QueueItem[]>([]);
    const [splitQueue, setSplitQueue] = useState<SplitItem[]>([]);
    const [processing, setProcessing] = useState(false);
    const [initialized, setInitialized] = useState(false);
    const cancelledRef = useRef<Set<string>>(new Set());

    // Listen for upload progress events from Rust
    useEffect(() => {
        let unlisten: UnlistenFn | undefined;
        listen<ProgressPayload>('upload-progress', (event) => {
            const payload = event.payload;
            setUploadQueue(q => q.map(i => {
                if (i.id === payload.id) {
                    return {
                        ...i,
                        progress: payload.percent,
                        uploaded_bytes: payload.uploaded_bytes,
                        total_bytes: payload.total_bytes,
                        speed_bytes_per_sec: payload.speed_bytes_per_sec,
                    };
                }
                return i;
            }));
        }).then(fn => { unlisten = fn; });
        return () => { unlisten?.(); };
    }, []);

    // Listen for part-specific progress events (split file uploads)
    useEffect(() => {
        let unlisten: UnlistenFn | undefined;
        listen<{
            parent_transfer_id: string;
            part_index: number;
            total_parts: number;
            percent: number;
            uploaded_bytes: number;
            total_bytes: number;
            speed_bytes_per_sec: number;
        }>('part-progress', (event) => {
            const payload = event.payload;
            console.log('[Upload] Part progress:', payload.parent_transfer_id, 'part', payload.part_index, payload.percent + '%');
            setUploadQueue(q => q.map(i => {
                if (i.id === payload.parent_transfer_id) {
                    return {
                        ...i,
                        partIndex: payload.part_index,
                        totalParts: payload.total_parts,
                        partProgress: payload.percent,
                        partUploadedBytes: payload.uploaded_bytes,
                        partTotalBytes: payload.total_bytes,
                        partSpeedBytesPerSec: payload.speed_bytes_per_sec,
                    };
                }
                return i;
            }));
        }).then(fn => { unlisten = fn; });
        return () => { unlisten?.(); };
    }, []);

    // Listen for split progress events from Rust
    useEffect(() => {
        let unlisten: UnlistenFn | undefined;
        listen<SplitProgressPayload>('split-progress', (event) => {
            const payload = event.payload;
            setSplitQueue(q => {
                const existing = q.findIndex(i => i.id === payload.id);
                if (existing >= 0) {
                    const updated = [...q];
                    updated[existing] = {
                        ...updated[existing],
                        status: payload.status as SplitItem['status'],
                        progress: payload.progress,
                        message: payload.message,
                    };
                    return updated;
                }
                return [...q, {
                    id: payload.id,
                    filename: payload.filename,
                    status: payload.status as SplitItem['status'],
                    progress: payload.progress,
                    message: payload.message,
                }];
            });

            // When split starts, mark the upload item as 'splitting'
            if (payload.status === 'splitting' || payload.status === 'zipping' || payload.status === 'partitioning') {
                setUploadQueue(q => q.map(i =>
                    i.id === payload.id ? { ...i, status: 'splitting' as const } : i
                ));
            }

            // When split completes, mark the upload item as 'uploading' and update parts info
            if (payload.status === 'success') {
                const partCount = parseInt(payload.message.match(/(\d+) parts/)?.[1] || '0');
                setUploadQueue(q => q.map(i =>
                    i.id === payload.id ? { ...i, status: 'uploading' as const, totalParts: partCount || undefined, partIndex: 0 } : i
                ));
                // Auto-dismiss split queue item after delay
                setTimeout(() => {
                    setSplitQueue(q => q.filter(i => i.id !== payload.id));
                }, 1500);
            }
        }).then(fn => { unlisten = fn; });
        return () => { unlisten?.(); };
    }, []);

    // Listen for part status events (which part is currently uploading)
    useEffect(() => {
        let unlisten: UnlistenFn | undefined;
        listen<PartStatusPayload>('upload-part-status', (event) => {
            const payload = event.payload;
            setUploadQueue(q => q.map(i =>
                i.id === payload.parentTransferId ? {
                    ...i,
                    partIndex: payload.partIndex,
                    totalParts: payload.totalParts,
                    status: 'uploading' as const,
                } : i
            ));
        }).then(fn => { unlisten = fn; });
        return () => { unlisten?.(); };
    }, []);

    useEffect(() => {
        if (!store || initialized) return;
        store.get<QueueItem[]>('uploadQueue').then((saved) => {
            if (saved && saved.length > 0) {
                const pending = saved.filter(i => i.status === 'pending');
                if (pending.length > 0) {
                    setUploadQueue(pending);
                    toast.info(`Restored ${pending.length} pending uploads`);
                }
            }
            setInitialized(true);
        });
    }, [store, initialized]);

    useEffect(() => {
        if (!store || !initialized) return;
        const pending = uploadQueue.filter(i => i.status === 'pending');
        store.set('uploadQueue', pending).then(() => store.save());
    }, [store, uploadQueue, initialized]);

    // Queue processor — only picks up 'pending' items
    // Items in 'splitting' status are handled by the backend (blocking call)
    useEffect(() => {
        if (processing) return;
        const nextItem = uploadQueue.find(i => i.status === 'pending');
        if (nextItem) {
            processItem(nextItem);
        }
    }, [uploadQueue, processing]);

    const processItem = async (item: QueueItem) => {
        setProcessing(true);
        setUploadQueue(q => q.map(i => i.id === item.id ? { ...i, status: 'uploading', progress: 0 } : i));
        try {
            await invoke('cmd_upload_file', { path: item.path, folderId: item.folderId, transferId: item.id });
            if (cancelledRef.current.has(item.id)) {
                cancelledRef.current.delete(item.id);
                setUploadQueue(q => q.map(i => i.id === item.id ? { ...i, status: 'cancelled' } : i));
            } else {
                setUploadQueue(q => q.map(i => i.id === item.id ? { ...i, status: 'success', progress: 100 } : i));
                queryClient.invalidateQueries({ queryKey: ['files', item.folderId] });
            }
        } catch (e) {
            const errStr = String(e);
            if (errStr.includes('cancelled')) {
                setUploadQueue(q => q.map(i => i.id === item.id ? { ...i, status: 'cancelled' } : i));
                cancelledRef.current.delete(item.id);
            } else if (cancelledRef.current.has(item.id)) {
                setUploadQueue(q => q.map(i => i.id === item.id ? { ...i, status: 'cancelled' } : i));
                cancelledRef.current.delete(item.id);
            } else {
                setUploadQueue(q => q.map(i => i.id === item.id ? { ...i, status: 'error', error: errStr } : i));
                toast.error(`Upload failed for ${item.path.split(/[\\/]/).pop()}: ${e}`);
            }
        } finally {
            setProcessing(false);
        }
    };

    const handleManualUpload = async () => {
        try {
            const selected = await open({ multiple: true, directory: false });
            if (selected) {
                const paths = Array.isArray(selected) ? selected : [selected];
                const newItems: QueueItem[] = paths.map((path: string) => ({
                    id: Math.random().toString(36).substr(2, 9),
                    path,
                    folderId: activeFolderId,
                    status: 'pending' as const
                }));
                setUploadQueue(prev => [...prev, ...newItems]);
                toast.info(`Queued ${paths.length} files for upload`);
            }
        } catch {
            toast.error("Failed to open file dialog");
        }
    };

    const cancelItem = async (id: string) => {
        const item = uploadQueue.find(i => i.id === id);
        if (item?.status === 'uploading' || item?.status === 'splitting') {
            cancelledRef.current.add(id);
            try {
                await invoke('cmd_cancel_transfer', { transferId: id });
            } catch (e) {
                console.error('Cancel transfer error:', e);
            }
        }
        // Mark as cancelled immediately for pending items
        setUploadQueue(q => q.map(i =>
            i.id === id && (i.status === 'pending' || i.status === 'uploading' || i.status === 'splitting')
                ? { ...i, status: 'cancelled' as const }
                : i
        ));
        // Also remove from split queue if applicable
        setSplitQueue(q => q.filter(i => i.id !== id));
    };

    const cancelAll = async () => {
        const active = uploadQueue.filter(i => i.status === 'uploading' || i.status === 'splitting');
        for (const item of active) {
            cancelledRef.current.add(item.id);
            try {
                await invoke('cmd_cancel_transfer', { transferId: item.id });
            } catch (e) {
                console.error('Cancel transfer error:', e);
            }
        }
        setUploadQueue(q => q
            .filter(i => i.status !== 'pending')
            .map(i => i.status === 'uploading' || i.status === 'splitting' ? { ...i, status: 'cancelled' as const } : i)
        );
        setSplitQueue([]);
        toast.info('All uploads cancelled');
    };

    const clearFinished = () => {
        setUploadQueue(q => q.filter(i => i.status !== 'success' && i.status !== 'error' && i.status !== 'cancelled'));
    };

    const removeSplitItem = (id: string) => {
        setSplitQueue(q => q.filter(i => i.id !== id));
    };

    const { isDragging } = useFileDrop();

    return {
        uploadQueue,
        setUploadQueue,
        splitQueue,
        handleManualUpload,
        cancelItem,
        cancelAll,
        clearFinished,
        removeSplitItem,
        isDragging
    };
}
