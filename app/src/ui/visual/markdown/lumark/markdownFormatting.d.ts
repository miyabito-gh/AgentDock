// 出典: Lumark 0.15.0 の src/lib/markdownFormatting.d.ts（純粋関数。そのままコピー）。
export interface MarkdownEdit {
  start: number;
  end: number;
  text: string;
  selectionStart: number;
  selectionEnd: number;
}
export interface MarkdownLink {
  start: number;
  end: number;
  label: string;
  url: string;
  title?: string;
}
export function createLinePrefixEdit(text: string, start: number, end: number, prefix: string): MarkdownEdit | null;
export function createHeadingLevelEdit(text: string, start: number, end: number, direction: number): MarkdownEdit | null;
export function createInlineFormatEdit(text: string, start: number, end: number, prefix: string, suffix?: string): MarkdownEdit | null;
export function findMarkdownLink(text: string, start: number, end?: number): MarkdownLink | null;
export function createMarkdownLinkEdit(text: string, start: number, end: number, label: string, url: string, existingLink?: MarkdownLink | null): MarkdownEdit;
