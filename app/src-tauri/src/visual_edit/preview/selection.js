(() => {
  if (window !== window.top) return;
  let mode = true, selected = [], hovered = null, overlay = null, overlayRoot = null, drag = null, band = null, suppressClick = false;
  const max = (v, n) => Array.from(String(v || '')).slice(0, n).join('');
  const selector = el => { const parts = []; for (let e = el; e && parts.length < 8; e = e.parentElement) { let p = e.localName; if (e.id) { p += '#' + CSS.escape(e.id); parts.unshift(p); break; } if (e.parentElement) p += ':nth-of-type(' + (Array.from(e.parentElement.children).filter(s => s.localName === e.localName).indexOf(e) + 1) + ')'; parts.unshift(p); } return max(parts.join(' > '), 1024); };
  const describe = e => { const r = e.getBoundingClientRect(); return { tag:max(e.localName,64), id:max(e.id,1024), className:max(e.getAttribute('class'),1024), text:max(e.textContent,1024), selector:selector(e), rect:{x:r.x,y:r.y,width:r.width,height:r.height} }; };
  const post = () => { selected = selected.filter(e => e.isConnected); let items = selected.slice(0,64).map(describe); let value = JSON.stringify({selection:items,truncated:items.length < selected.length}); while (new TextEncoder().encode(value).length > 65000 && items.length) { items.pop(); value = JSON.stringify({selection:items,truncated:items.length < selected.length}); } try { window.chrome.webview.postMessage(value); } catch (_) {} draw(); };
  function draw() { if (!document.documentElement) return; if (!overlay || !overlay.isConnected) { overlay = document.createElement('div'); overlay.style.cssText='position:fixed;inset:0;z-index:2147483647;pointer-events:none;'; overlayRoot=overlay.attachShadow({mode:'closed'}); document.documentElement.appendChild(overlay); } // Draw outside page styles, with no changes to source elements.

    const root=overlayRoot; root.replaceChildren();
    for (const e of [...selected, ...(mode && hovered && !selected.includes(hovered) ? [hovered] : [])]) { if (!e.isConnected) continue; const r=e.getBoundingClientRect(), box=document.createElement('div'); box.style.cssText=`position:fixed;pointer-events:none;box-sizing:border-box;left:${r.x}px;top:${r.y}px;width:${r.width}px;height:${r.height}px;border:2px solid ${selected.includes(e)?'#8b5cf6':'#38bdf8'};background:${selected.includes(e)?'#8b5cf612':'transparent'};`; root.appendChild(box); }
    if (band) { const b=document.createElement('div'); b.style.cssText=`position:fixed;pointer-events:none;box-sizing:border-box;left:${band.x}px;top:${band.y}px;width:${band.w}px;height:${band.h}px;border:1.5px dashed #8b5cf6;background:#8b5cf61a;`; root.appendChild(b); }
  }
  // ドラッグした範囲に十分入っている要素を、親子の重複なく選ぶ（大きな外枠は含めない）。
  const pick = rect => {
    const area = r => Math.max(0, r.width) * Math.max(0, r.height), skip = new Set(['HTML','BODY','SCRIPT','STYLE','LINK','META','HEAD','TITLE','NOSCRIPT','BR']);
    const inside = [], partial = [];
    for (const e of document.body ? document.body.querySelectorAll('*') : []) {
      if (skip.has(e.tagName) || e === overlay) continue;
      const r = e.getBoundingClientRect(), a = area(r); if (a < 16 || r.width < 3 || r.height < 3) continue;
      const w = Math.min(r.right, rect.x + rect.w) - Math.max(r.left, rect.x), h = Math.min(r.bottom, rect.y + rect.h) - Math.max(r.top, rect.y);
      if (w <= 0 || h <= 0) continue;
      const ratio = (w * h) / a;
      if (ratio >= 0.9) inside.push(e); else if (ratio >= 0.5) partial.push(e);
    }
    // 範囲に完全に入る要素は、親子の重複なく一番外側だけを選ぶ。なければ、部分的に入る要素の一番内側を選ぶ。
    if (inside.length) { const set = new Set(inside); return inside.filter(e => { for (let p = e.parentElement; p; p = p.parentElement) if (set.has(p)) return false; return true; }).slice(0, 64); }
    if (partial.length) { const set = new Set(partial); return partial.filter(e => !partial.some(o => o !== e && e.contains(o))).slice(0, 64); }
    const c = document.elementFromPoint(rect.x + rect.w / 2, rect.y + rect.h / 2);
    return c && !skip.has(c.tagName) && c !== overlay ? [c] : [];
  };
  const target = event => { const e=event.composedPath()[0]; return e instanceof Element && e !== overlay && !overlay?.contains(e) ? e : null; };
  window.addEventListener('pointermove', e => { if (mode) { hovered=target(e); draw(); } },true);
  window.addEventListener('pointerdown', e => { if (mode && e.button === 0 && target(e)) drag = { x: e.clientX, y: e.clientY, active: false, ctrl: e.ctrlKey }; },true);
  window.addEventListener('pointermove', e => { if (!drag) return; const dx = e.clientX - drag.x, dy = e.clientY - drag.y; if (!drag.active && Math.hypot(dx, dy) < 6) return; drag.active = true; band = { x: Math.min(drag.x, e.clientX), y: Math.min(drag.y, e.clientY), w: Math.abs(dx), h: Math.abs(dy) }; hovered = null; draw(); },true);
  window.addEventListener('pointerup', e => { const d = drag; drag = null; const far = d && Math.hypot(e.clientX - d.x, e.clientY - d.y) >= 6; if (!d || !(d.active || far)) { band = null; return; } const rect = { x: Math.min(d.x, e.clientX), y: Math.min(d.y, e.clientY), w: Math.abs(e.clientX - d.x), h: Math.abs(e.clientY - d.y) }; band = null; suppressClick = true; setTimeout(() => { suppressClick = false; }, 0); const found = pick(rect); selected = d.ctrl ? [...new Set([...selected, ...found])].slice(0, 64) : found; post(); },true);
  window.addEventListener('pointercancel', () => { drag = null; band = null; draw(); },true);
  window.addEventListener('dragstart', e => { if (mode) e.preventDefault(); },true);
  window.addEventListener('selectstart', e => { if (mode && drag) e.preventDefault(); },true);
  window.addEventListener('click', e => { if (suppressClick) { e.preventDefault(); e.stopImmediatePropagation(); return; } if (!mode) return; const el=target(e); if (!el) return; e.preventDefault(); e.stopImmediatePropagation(); if (e.ctrlKey) selected=selected.includes(el)?selected.filter(x=>x!==el):[...selected,el].slice(0,64); else selected=[el]; post(); },true);
  window.addEventListener('keydown', e => { if (e.key==='Escape') { selected=[]; hovered=null; post(); } },true);
  window.addEventListener('scroll',draw,true); window.addEventListener('resize',draw);
  window.addEventListener('agentdock-preview-action', e => { switch(e.detail) { case 'select':mode=true;break;case 'interact':mode=false;hovered=null;break;case 'parent':if (selected.length) { const primary=selected[selected.length-1]; const parent=primary.parentElement; if(parent) selected=[...new Set([...selected.slice(0,-1),parent])]; }post();break;case 'clear':selected=[];hovered=null;post();break; } draw(); });
  document.addEventListener('DOMContentLoaded', () => { draw(); post(); },{once:true});
})();