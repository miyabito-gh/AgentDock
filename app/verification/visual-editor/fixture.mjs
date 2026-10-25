const image=document.querySelector('#image');
const report=()=>parent.postMessage({kind:'relative-assets',module:true,css:getComputedStyle(document.querySelector('#styled')).color,image:image.complete&&image.naturalWidth>0},'*');
image.complete?report():image.addEventListener('load',report,{once:true});