import { createRoot } from "react-dom/client";
import { sanitizeMarkdownHtml } from "../../src/ui/visual/markdown/sanitize";
import { renderMarkdown } from "../../src/ui/visual/markdown/render";
import { MarkdownPreview } from "../../src/ui/visual/markdown/MarkdownPreview";
const src = '[ext](https://example.com/x)\n\n<img src="img/a.png" usemap="#m" id="root" name="n"><map name="m"><area shape="rect" coords="0,0,500,500" href="https://example.com/area"></map>\n\n<div id="pane-dock" name="x" href="https://e.com" background="https://e.com/b" poster="https://e.com/p">div</div>\n\n<a id="pane-dock" href="https://example.com/a">raw a</a>\n';
const html = sanitizeMarkdownHtml(renderMarkdown(src).html);
(window as any).__html = html;
createRoot(document.getElementById("root")!).render(<MarkdownPreview html={html} text={src} rootPath="C:/d" path="C:/d/a.md" epoch={0} selection={[]} onSelect={() => {}} />);
