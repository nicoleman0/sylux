export type UploadItem =
  | { kind: 'folder'; path: string[] }
  | { kind: 'file'; path: string[]; file: File };

type DropEntry = {
  name: string;
  isFile: boolean;
  isDirectory: boolean;
  file?: (success: (file: File) => void, failure: (error: DOMException) => void) => void;
  createReader?: () => { readEntries: (success: (entries: DropEntry[]) => void, failure: (error: DOMException) => void) => void };
};

function validName(name: string): void {
  if (!name || name === '.' || name === '..' || /[/\\\x00-\x1f\x7f]/.test(name)) {
    throw new Error('A dropped item has an unsupported name. Rename it and try again.');
  }
}

export function filesToUpload(files: File[]): UploadItem[] {
  return files.map((file) => {
    validName(file.name);
    return { kind: 'file', path: [file.name], file };
  });
}

// The browser protects DataTransfer after the drop handler yields.
export function snapshotDesktopDrop(data: DataTransfer): (canceled?: () => boolean) => Promise<UploadItem[]> {
  const sources: Array<DropEntry | File> = [];
  const items = Array.from(data.items).filter((item) => item.kind === 'file');
  if (items.length) {
    for (const item of items) {
      const entry = item.webkitGetAsEntry?.() as DropEntry | null;
      const source = entry ?? item.getAsFile();
      if (!source) throw new Error('A dropped item could not be read. Try Choose files instead.');
      sources.push(source);
    }
  } else sources.push(...Array.from(data.files));

  return async (canceled = () => false) => {
    const result: UploadItem[] = [];
    async function visit(source: DropEntry | File, parent: string[]): Promise<void> {
      if (canceled()) throw new DOMException('Upload canceled.', 'AbortError');
      validName(source.name);
      const path = [...parent, source.name];
      if (source instanceof File) {
        // Some browsers expose an unreadable directory as a zero-byte File.
        await (source.size === 0 ? source.arrayBuffer() : source.slice(0, 1).arrayBuffer());
        result.push({ kind: 'file', path, file: source });
      } else if (source.isFile && source.file) {
        const file = await new Promise<File>((resolve, reject) => source.file!(resolve, reject));
        result.push({ kind: 'file', path, file });
      } else if (source.isDirectory && source.createReader) {
        result.push({ kind: 'folder', path });
        const reader = source.createReader();
        for (;;) {
          const entries = await new Promise<DropEntry[]>((resolve, reject) => reader.readEntries(resolve, reject));
          if (canceled()) throw new DOMException('Upload canceled.', 'AbortError');
          if (!entries.length) break;
          for (const entry of entries) await visit(entry, path);
        }
      } else throw new Error(`Could not read ${path.join('/')}. Try Choose files instead.`);
    }
    try {
      for (const source of sources) await visit(source, []);
    } catch (error) {
      if (error instanceof DOMException && error.name === 'AbortError') throw error;
      if (error instanceof Error && error.message.includes('unsupported name')) throw error;
      throw new Error('A dropped file or folder could not be read. Nothing was uploaded. Check access and try again.');
    }
    return result;
  };
}
