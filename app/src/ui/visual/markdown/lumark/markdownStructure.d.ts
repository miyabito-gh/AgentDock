// 出典: Lumark 0.15.0 の src/lib/markdownStructure.d.ts（純粋関数。上のコピーに合わせて、使う宣言だけ）。
export interface MarkdownStructureEdit {
  start: number;
  end: number;
  text: string;
  selectionStart: number;
  selectionEnd: number;
}
export function createIndentEdit(text: string, start: number, end: number, tabSize: number, outdent?: boolean): MarkdownStructureEdit | null;
export function isInsideCodeFence(text: string, position: number): boolean;
export function getMarkdownLineContexts(text: string): { start: number; end: number; text: string; code: boolean }[];
