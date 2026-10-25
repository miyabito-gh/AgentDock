(() => {
  if (window !== window.top) return;
  let mode = true, selected = [], hovered = null, overlay = null, overlayRoot = null;
  const max = (v, n) => Array.from(String(v || '')).slice(0, n).join('');
  const selector = el => { const parts = []; for (let e = el; e && parts.length < 8; e = e.parentElement) { let p = e.localName; if (e.id) { p += '#' + CSS.escape(e.id); parts.unshift(p); break; } if (e.parentElement) p += ':nth-of-type(' + (Array.from(e.parentElement.children).filter(s => s.localName === e.localName).indexOf(e) + 1) + ')'; parts.unshift(p); } return max(parts.join(' > '), 1024); };
  const describe = e => { const r = e.getBoundingClientRect(); return { tag:max(e.localName,64), id:max(e.id,1024), className:max(e.getAttribute('class'),1024), text:max(e.textContent,1024), selector:selector(e), rect:{x:r.x,y:r.y,width:r.width,height:r.height} }; };
  const post = () => { selected = selected.filter(e => e.isConnected); let items = selected.slice(0,64).map(describe); let value = JSON.stringify({selection:items,truncated:items.length < selected.length}); while (new TextEncoder().encode(value).length > 65000 && items.length) { items.pop(); value = JSON.stringify({selection:items,truncated:items.length < selected.length}); } try { window.chrome.webview.postMessage(value); } catch (_) {} draw(); };
  function draw() { if (!document.documentElement) return; if (!overlay || !overlay.isConnected) { overlay = document.createElement('div'); overlay.style.cssText='position:fixed;inset:0;z-index:2147483647;pointer-events:none;'; overlayRoot=overlay.attachShadow({mode:'closed'}); document.documentElement.appendChild(overlay); } // Draw outside page styles, with no changes to source elements.

    const root=overlayRoot; root.replaceChildren();
    for (const e of [...selected, ...(mode && hovered && !selected.includes(hovered) ? [hovered] : [])]) { if (!e.isConnected) continue; const r=e.getBoundingClientRect(), box=document.createElement('div'); box.style.cssText=`position:fixed;pointer-events:none;box-sizing:border-box;left:${r.x}px;top:${r.y}px;width:${r.width}px;height:${r.height}px;border:2px solid ${selected.includes(e)?'#8b5cf6':'#38bdf8'};background:${selected.includes(e)?'#8b5cf612':'transparent'};`; root.appendChild(box); }
  }
  const target = event => { const e=event.composedPath()[0]; return e instanceof Element && e !== overlay && !overlay?.contains(e) ? e : null; };
  window.addEventListener('pointermove', e => { if (mode) { hovered=target(e); draw(); } },true);
  window.addEventListener('click', e => { if (!mode) return; const el=target(e); if (!el) return; e.preventDefault(); e.stopImmediatePropagation(); if (e.ctrlKey) selected=selected.includes(el)?selected.filter(x=>x!==el):[...selected,el].slice(0,64); else selected=[el]; post(); },true);
  window.addEventListener('keydown', e => { if (e.key==='Escape') { selected=[]; hovered=null; post(); } },true);
  window.addEventListener('scroll',draw,true); window.addEventListener('resize',draw);
  window.addEventListener('agentdock-preview-action', e => { switch(e.detail) { case 'select':mode=true;break;case 'interact':mode=false;hovered=null;break;case 'parent':if (selected.length) { const primary=selected[selected.length-1]; const parent=primary.parentElement; if(parent) selected=[...new Set([...selected.slice(0,-1),parent])]; }post();break;case 'clear':selected=[];hovered=null;post();break; } draw(); });
  document.addEventListener('DOMContentLoaded', () => { draw(); post(); },{once:true});
})();