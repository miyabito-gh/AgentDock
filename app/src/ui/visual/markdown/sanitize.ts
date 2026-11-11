// 描画したHTMLの安全処理（DOMPurify）。ブラウザのDOMが要るので node のテスト対象外。
// 方針（Lumark の markdownSecurity 相当）: 外部へ自動で通信しない。画像は data: とフォルダ内の相対パス（ホスト経由）だけ。
// リンクは移動させない（href を外し data-href に退避）。スクリプト・フォーム・埋め込み・style は除く。
import DOMPurify from "dompurify";
import { safeDataImageSource, safeLinkTarget } from "./lumarkSecurity.ts";
import { anchorKey } from "./anchor.ts";

let installed = false;
function install(): void {
  if (installed) return;
  installed = true;
  DOMPurify.addHook("afterSanitizeAttributes", (node) => {
    if (!(node instanceof Element)) return;
    const tag = node.tagName;
    // 保険: A・IMG 以外の要素は、移動や読み込みにつながるURL属性を持たせない。
    if (tag !== "A" && tag !== "IMG") for (const attr of URL_ATTRS) node.removeAttribute(attr);
    if (tag === "IMG") {
      const src = node.getAttribute("src") ?? "";
      node.removeAttribute("src");
      node.removeAttribute("srcset");
      const data = safeDataImageSource(src);
      if (data) node.setAttribute("src", data);
      else if (/^[a-z][a-z0-9+.-]*:/i.test(src) || src.startsWith("//")) node.setAttribute("data-blocked", "external");
      else if (src) node.setAttribute("data-local-src", src);
    } else if (tag === "A") {
      const href = node.getAttribute("href");
      node.removeAttribute("href");
      node.removeAttribute("target");
      if (href === null) return;
      const safe = safeLinkTarget(href);
      if (safe?.startsWith("#")) { node.setAttribute("data-anchor", anchorKey(safe.slice(1))); node.setAttribute("tabindex", "0"); node.setAttribute("role", "link"); }
      else node.setAttribute("title", `リンク先: ${href}（プレビューでは開きません）`);
      node.classList.add("md-link-off");
    } else if (tag === "INPUT") {
      if (node.getAttribute("type") !== "checkbox") { node.remove(); return; }
      node.setAttribute("disabled", "");
      node.setAttribute("tabindex", "-1");
    }
  });
}

const URL_ATTRS = ["href", "xlink:href", "src", "srcset", "action", "formaction", "poster", "background", "data", "ping", "longdesc", "usemap"];
const FORBID_TAGS = ["area", "map", "style", "script", "form", "button", "textarea", "select", "option", "link", "meta", "base", "iframe", "object", "embed", "audio", "video", "source", "template"];
const FORBID_ATTR = ["style", "srcset", "ping", "formaction", "action", "target", "class", "usemap", "ismap", "id", "name"];

export function sanitizeMarkdownHtml(html: string): string {
  install();
  return DOMPurify.sanitize(html, { USE_PROFILES: { html: true }, FORBID_TAGS, FORBID_ATTR, ALLOW_DATA_ATTR: true, KEEP_CONTENT: true });
}
