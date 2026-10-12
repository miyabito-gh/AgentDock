// チャット一覧の追加取得（プロジェクト見出し・続きの読込み）と通知文の純粋ロジック。読み取りのみ。

/** 見出しごとの取得で読むページ数の上限（無限に読まない）。 */
export const MAX_FETCH_PAGES = 20;

export interface PageLike<T> { items: T[]; nextCursor: string | null }

export interface FetchAllResult<T> {
  items: T[];
  /** 読んだページ数。 */
  pages: number;
  /** 上限に達して、続きが残っている。 */
  truncated: boolean;
  /** 途中で失敗した理由（失敗しても取れた分は items に残る。成功と偽らない）。 */
  error: string | null;
}

/** nextCursor を辿って最大 maxPages ページ読む。途中の失敗では、それまでに取れた分を返す。 */
export async function fetchAllPages<T>(fetchPage: (cursor: string | null) => Promise<PageLike<T>>, maxPages = MAX_FETCH_PAGES, errText: (e: unknown) => string = String): Promise<FetchAllResult<T>> {
  const items: T[] = [];
  let cursor: string | null = null;
  let pages = 0;
  for (;;) {
    let page: PageLike<T>;
    try { page = await fetchPage(cursor); }
    catch (e) { return { items, pages, truncated: false, error: errText(e) }; }
    pages++;
    items.push(...page.items);
    cursor = page.nextCursor;
    if (!cursor) return { items, pages, truncated: false, error: null };
    if (pages >= maxPages) return { items, pages, truncated: true, error: null };
  }
}

/** 取得した ID の総数（重複除く）と、取得前に一覧に無かった新規の数。 */
export function countFetched(fetchedIds: string[], knownIds: Set<string>): { total: number; fresh: number } {
  const uniq = new Set(fetchedIds);
  let fresh = 0;
  for (const id of uniq) if (!knownIds.has(id)) fresh++;
  return { total: uniq.size, fresh };
}

/** 見出しごとの取得の完了通知。失敗時は理由を付け、取れた分を示す。 */
export function projectFetchMessage(label: string, r: { total: number; fresh: number; truncated: boolean; error: string | null }): string {
  const base = `「${label}」のチャットを${r.total}件取得しました（うち新規${r.fresh}件）`;
  if (r.error) return `「${label}」のチャットを取得できませんでした: ${r.error}${r.total ? `（途中までの${r.total}件は取得済み、うち新規${r.fresh}件）` : ""}`;
  return `${base}${r.truncated ? `。ページ数の上限（${MAX_FETCH_PAGES}ページ）に達したため続きがあるかもしれません` : ""}。フォルダの完全一致のみで、下位フォルダのチャットは含まれません`;
}

/** 「続きを読み込む」の完了通知。 */
export function loadMoreMessage(r: { total: number; fresh: number; more: boolean }): string {
  return `続きを${r.total}件取得しました（うち新規${r.fresh}件）${r.more ? "。まだ続きがあります" : "。これで最後です"}`;
}

/** 「プロジェクト一覧を更新」の完了通知（最新の振り分けの件数）。 */
export function projectUpdateMessage(c: { projects: number; confirmed: number; guessed: number; none: number; unknown: number }): string {
  return `プロジェクト${c.projects}件を更新しました（確定${c.confirmed}件／推定${c.guessed}件／プロジェクトなし${c.none}件${c.unknown ? `／フォルダ未確認${c.unknown}件` : ""}）`;
}
