import { useState, type DragEvent } from 'react';
import { motecStore } from './useMotecStore';

export interface MotecDropZone {
  readonly dragActive: boolean;
  readonly dropZoneProps: {
    readonly onDragEnter: (e: DragEvent<HTMLElement>) => void;
    readonly onDragOver: (e: DragEvent<HTMLElement>) => void;
    readonly onDragLeave: (e: DragEvent<HTMLElement>) => void;
    readonly onDrop: (e: DragEvent<HTMLElement>) => void;
  };
}

function hasFiles(e: DragEvent<HTMLElement>): boolean {
  return Array.from(e.dataTransfer.types).includes('Files');
}

/** Drop-zone handlers for a container (the pitwall graph panel): a dropped .ld file loads as the MoTeC reference. */
export function useMotecDropZone(): MotecDropZone {
  const [dragActive, setDragActive] = useState(false);
  return {
    dragActive,
    dropZoneProps: {
      onDragEnter: (e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        setDragActive(true);
      },
      onDragOver: (e) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
      },
      onDragLeave: (e) => {
        // Only when the pointer leaves the zone itself, not a child.
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setDragActive(false);
      },
      onDrop: (e) => {
        e.preventDefault();
        setDragActive(false);
        const files = e.dataTransfer.files;
        if (files.length > 0) void motecStore.loadFiles(files);
      },
    },
  };
}
