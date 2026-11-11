// 出典: Lumark 0.15.0 の src/lib/markdownSecurity.ts から、純粋関数だけをコピー（safeLinkTarget / normalizeLocalMarkdownPath / createHeadingSlugger）。
// safeDataImageSource は同ファイルの safeExternalImageSource から、外部URLの許可を除いて data: 画像だけを許す形に改変（外部へ自動で通信しないため）。
// テスト: lumarkSecurity.test.ts（Lumark の markdownSecurity.test.mjs から該当分を移植）。

const EXTERNAL_LINK_PROTOCOLS = new Set(["http:", "https:", "mailto:"]);
const SAFE_DATA_IMAGE = /^data:image\/(?:png|jpeg|gif|webp|bmp);base64,[a-z0-9+/=\s]+$/i;

function parsedProtocol(value: string): string | null {
  try {
    return new URL(value).protocol.toLowerCase();
  } catch {
    return null;
  }
}

export function safeLinkTarget(href: string | null | undefined): string | null {
  const value = href?.trim();
  if (!value) return null;
  if (value.startsWith("#")) return value;

  const protocol = parsedProtocol(value);
  return protocol && EXTERNAL_LINK_PROTOCOLS.has(protocol) ? value : null;
}

/** data: 画像（既知形式のbase64）だけを許す。http(s)等の外部画像は読み込まない。 */
export function safeDataImageSource(href: string | null | undefined): string | null {
  const value = href?.trim();
  return value && SAFE_DATA_IMAGE.test(value) ? value : null;
}

export function normalizeLocalMarkdownPath(href: string | null | undefined): string | null {
  const raw = href?.trim();
  if (!raw || raw.includes("\0")) return null;

  let decoded: string;
  try {
    decoded = decodeURIComponent(raw);
  } catch {
    return null;
  }

  const pathOnly = decoded.split(/[?#]/, 1)[0].replace(/\\/g, "/");
  if (
    !pathOnly ||
    pathOnly.startsWith("/") ||
    pathOnly.startsWith("//") ||
    /^[a-z]:/i.test(pathOnly) ||
    parsedProtocol(pathOnly)
  ) {
    return null;
  }

  const parts: string[] = [];
  for (const part of pathOnly.split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
      continue;
    }
    parts.push(part);
  }

  return parts.length > 0 ? parts.join("/") : null;
}

export function createHeadingSlugger(): (value: string) => string {
  const occurrences = new Map<string, number>();

  return (value: string) => {
    const base =
      value
        .normalize("NFKC")
        .toLowerCase()
        .replace(/<[^>]*>/g, "")
        .replace(/[^\p{Letter}\p{Number}\s_-]/gu, "")
        .trim()
        .replace(/\s+/g, "-") || "section";
    const occurrence = occurrences.get(base) ?? 0;
    occurrences.set(base, occurrence + 1);
    return occurrence === 0 ? base : `${base}-${occurrence}`;
  };
}
