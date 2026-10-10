const ICON: Record<string, string> = {
  plus: '<path d="M12 5v14M5 12h14"/>', search: '<circle cx="11" cy="11" r="6"/><path d="m20 20-4.5-4.5"/>',
  archive: '<rect x="3" y="4" width="18" height="5" rx="1"/><path d="M5 9v10h14V9M10 13h4"/>',
  send: '<path d="M12 19V5M6 11l6-6 6 6"/>', stop: '<rect x="6" y="6" width="12" height="12" rx="1.5"/>',
  clip: '<path d="m20 11-8.5 8.5a5 5 0 0 1-7-7L13 4a3.3 3.3 0 0 1 4.7 4.7L9.2 17.2a1.7 1.7 0 0 1-2.4-2.4L14.5 7"/>',
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  folderOpen: '<path d="M3 18V7a2 2 0 0 1 2-2h4l2 2h7a2 2 0 0 1 2 2v1"/><path d="M3 18l2.5-7a1.5 1.5 0 0 1 1.4-1H21l-2.6 7.3a1.5 1.5 0 0 1-1.4.7H5a2 2 0 0 1-2-1z"/>',
  folderUnknown: '<path stroke-dasharray="3 2" d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M10.2 11.2a1.9 1.9 0 1 1 2.7 1.7c-.6.3-.9.7-.9 1.4M12 16.3v.4"/>',
  pin: '<path d="M9 4h6l-1 6 3 3H7l3-3zM12 13v7"/>',
  dock: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M15 4v16"/>', list: '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M9 4v16"/>',
  copy: '<rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>', min: '<path d="M6 12h12"/>', max: '<rect x="6" y="6" width="12" height="12" rx="1"/>',
  branch: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 7v10M18 10c0 4-6 3-12 7"/>',
  diff: '<path d="M8 4v10M3 9h10M11 20h10"/>', alert: '<path d="M12 4 2.5 20h19z"/><path d="M12 10v4M12 17v.5"/>',
  check: '<path d="m5 12 4.5 4.5L19 7"/>', chat: '<path d="M4 5h16v11H9l-5 4z"/>',
  more: '<circle cx="5" cy="12" r="1.3"/><circle cx="12" cy="12" r="1.3"/><circle cx="19" cy="12" r="1.3"/>',
  help: '<circle cx="12" cy="12" r="9"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.6 2.2c-.7.4-1.1.9-1.1 1.8M12 17v.5"/>',
  refresh: '<path d="M20 11a8 8 0 0 0-14-4.5L4 9M4 4v5h5M4 13a8 8 0 0 0 14 4.5L20 15M20 20v-5h-5"/>',
};

export function Icon({ name }: { name: string }) {
  return <svg className="i" viewBox="0 0 24 24" aria-hidden="true" dangerouslySetInnerHTML={{ __html: ICON[name] ?? "" }} />;
}

export function Flag({ c }: { c: string }) {
  return (
    <svg viewBox="0 0 12 12" aria-hidden="true">
      <path className={`flag ${c === "unk" ? "unk" : ""}`} d="M2 1.5v9h1.2V7.2L10.5 4.4 3.2 1.5z" />
    </svg>
  );
}
