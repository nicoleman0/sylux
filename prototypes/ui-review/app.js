const app = document.querySelector('#app');
const dialog = document.querySelector('#dialog');
const menuRoot = document.querySelector('#menu-root');
const icons = {
 logo:'<path d="M4 5h7l3 3h6v11H4z"/><path d="M4 9h16M8 13h8M8 16h5"/>',
 folder:'<path d="M3 7V5h6l2 2h10v12H3z"/>', file:'<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v5h4M9 12h6M9 16h6"/>',
 image:'<rect x="4" y="3" width="16" height="18" rx="1"/><circle cx="9" cy="8" r="1.5"/><path d="m4 17 5-5 4 4 3-3 4 4"/>',
 archive:'<path d="M6 3h12v18H6zM10 3v3h4v3h-4v3h4v4h-4v3"/>',
 trash:'<path d="M3 6h18M9 6V3h6v3M6 6l1 15h10l1-15M10 10v7M14 10v7"/>',
 user:'<circle cx="12" cy="8" r="4"/><path d="M4 21v-2a8 8 0 0 1 16 0v2"/>',
 settings:'<path d="m10 3-.7 2.4-2.2 1-2.3-.6-2 3.4 1.7 1.8v2l-1.7 1.8 2 3.4 2.3-.6 2.2 1L10 21h4l.7-2.4 2.2-1 2.3.6 2-3.4-1.7-1.8v-2l1.7-1.8-2-3.4-2.3.6-2.2-1L14 3z"/><circle cx="12" cy="12" r="3"/>',
 moon:'<path d="M20 14A8 8 0 0 1 10 4a8 8 0 1 0 10 10z"/>',sun:'<circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5"/>',
 plus:'<path d="M12 5v14M5 12h14"/>',upload:'<path d="M12 16V3m-5 5 5-5 5 5M4 15v6h16v-6"/>',chevron:'<path d="m9 5 7 7-7 7"/>',
 more:'<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',close:'<path d="m6 6 12 12M6 18 18 6"/>',
 edit:'<path d="m4 16-1 5 5-1L20 8l-4-4zM13 7l4 4"/>',move:'<path d="M3 7V5h6l2 2h10v12H3zM7 13h10m-3-3 3 3-3 3"/>',
 download:'<path d="M12 3v13m-5-5 5 5 5-5M4 17v4h16v-4"/>',restore:'<path d="M3 4v6h6M3 10a9 9 0 1 1 0 5"/>',
 logout:'<path d="M9 3H4v18h5M10 12h11m-4-4 4 4-4 4"/>',check:'<path d="m5 12 4 4L19 6"/>',key:'<circle cx="8" cy="8" r="5"/><path d="m12 12 9 9m-3-3 3-3m-6 0 3-3"/>',eye:'<path d="M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>'
};
const svg = (name) => `<svg viewBox="0 0 24 24" aria-hidden="true">${icons[name] || icons.file}</svg>`;
const esc = (v) => String(v).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const sample = () => [
 {id:'documents',name:'Documents',type:'folder',parent:null},
 {id:'photos',name:'Photos',type:'folder',parent:null},
 {id:'projects',name:'Projects',type:'folder',parent:null},
 {id:'shared-notes',name:'Shared notes',type:'folder',parent:null},
 {id:'readme',name:'Read me.txt',type:'file',parent:null,size:1240,content:'Welcome to your files\n\nThis is a quiet place to keep the things you need.\n\nDocuments   Records, notes and reference material\nPhotos      A few moments from the coast\nProjects    Work in progress\n\nYou are viewing synthetic files in the Sylux UI prototype.\nChanges last until this page is reloaded.'},
 {id:'archive',name:'Autumn archive.zip',type:'file',parent:null,size:18467328},
 {id:'packing',name:'Packing list.txt',type:'file',parent:null,size:860,content:'Weekend away\n\nClothes\n  Walking boots\n  Rain jacket\n  Warm layers\n\nIn the bag\n  Notebook and pen\n  Camera\n  Charging cables\n  Water bottle\n\nBefore leaving\n  Check the forecast\n  Download the route'},
 {id:'household',name:'Household',type:'folder',parent:'documents'},
 {id:'notes',name:'Notes',type:'folder',parent:'documents'},
 {id:'budget',name:'Household budget.xlsx',type:'file',parent:'documents',size:28467},
 {id:'garden',name:'Garden plans.txt',type:'file',parent:'documents',size:2160,content:'Garden plans\n\nSpring\n  Clear the raised beds\n  Start tomatoes indoors\n  Repair the cold frame\n\nSummer\n  Plant herbs by the kitchen door\n  Add a water butt beside the shed\n\nAutumn\n  Collect leaves for compost\n  Plant bulbs along the path'},
 {id:'manual',name:'Workshop manual.pdf',type:'file',parent:'documents',size:2437120},
 {id:'coast',name:'Coastal walk.jpg',type:'file',parent:'photos',size:4823440},
 {id:'harbour',name:'Harbour at dusk.jpg',type:'file',parent:'photos',size:3614110},
 {id:'project-notes',name:'Project notes.txt',type:'file',parent:'projects',size:1536,content:'Project notes\n\nA small shelf for the hallway.\n\nDimensions\n  Width: 80 cm\n  Depth: 22 cm\n  Height: 95 cm\n\nNext steps\n  Measure the skirting board\n  Choose the timber\n  Make a cutting list'},
 {id:'old-list',name:'Old shopping list.txt',type:'file',parent:'documents',size:420,trashed:true,content:'A previous shopping list. This is sample content.'},
 {id:'draft',name:'Draft sketch.png',type:'file',parent:'projects',size:782400,trashed:true}
];
const initialAccounts = () => [
 {id:'alex',name:'Alex Morgan',username:'alex',role:'Administrator',enabled:true},
 {id:'jamie',name:'Jamie Chen',username:'jamie',role:'User',enabled:true},
 {id:'sam',name:'Sam Taylor',username:'sam',role:'User',enabled:false}
];
let files = sample();
let accounts = initialAccounts();
let currentUser = accounts[0];
let chosenFile = null;
let menuTrigger = null;
let dialogTrigger = null;
let toastTimer;
let instanceName = 'Sylux';
let savedTheme;
try { instanceName = localStorage.getItem('sylux-prototype-name') || 'Sylux'; savedTheme = localStorage.getItem('sylux-prototype-theme'); } catch {}
if (savedTheme === 'light' || savedTheme === 'dark') document.documentElement.dataset.theme = savedTheme;
const isDark = () => document.documentElement.dataset.theme ? document.documentElement.dataset.theme === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches;
const themeButton = () => `<button class="icon-button" data-action="theme" aria-label="Switch to ${isDark() ? 'light' : 'dark'} theme" title="Switch to ${isDark() ? 'light' : 'dark'} theme">${svg(isDark() ? 'sun' : 'moon')}</button>`;
const brand = () => `<a class="brand" href="#/files" aria-label="${esc(instanceName)}, go to Files">${svg('logo')}<span>${esc(instanceName)}</span></a>`;
const formatSize = (bytes) => bytes < 1024 ? `${bytes} B` : bytes < 1048576 ? `${(bytes/1024).toFixed(bytes < 10240 ? 1 : 0)} KB` : `${(bytes/1048576).toFixed(1)} MB`;
const fileIcon = (file) => file.type === 'folder' ? 'folder' : /\.(jpg|png)$/i.test(file.name) ? 'image' : /\.zip$/i.test(file.name) ? 'archive' : 'file';
const pathOf = (id) => {
 const result = []; let node = files.find(f => f.id === id); const visited = new Set();
 while (node && !visited.has(node.id)) { result.unshift(node); visited.add(node.id); node = files.find(f => f.id === node.parent); }
 return result;
};
const activeRoute = () => location.hash.slice(1) || '/files';
function notify(message) {
 const el = document.querySelector('#toast'); el.textContent = message; el.hidden = false;
 clearTimeout(toastTimer); toastTimer = setTimeout(() => {el.hidden = true;}, 4500);
}
function closeMenu(restoreFocus = false) {menuTrigger?.setAttribute('aria-expanded','false'); menuRoot.innerHTML = ''; if (restoreFocus) menuTrigger?.focus(); menuTrigger = null;}
function showDialog(title, body, onSubmit, extraClass = '') {
 closeMenu(); dialogTrigger = document.activeElement;
 dialog.className = extraClass;
 dialog.innerHTML = `<div class="dialog-top"><h2 id="dialog-title">${esc(title)}</h2><button class="icon-button" data-action="close-dialog" aria-label="Close dialog">${svg('close')}</button></div><div class="dialog-body">${body}</div>`;
 const form = dialog.querySelector('form');
 if (form) form.addEventListener('submit', event => {event.preventDefault(); onSubmit?.(new FormData(form), form);});
 dialog.showModal();
 requestAnimationFrame(() => (dialog.querySelector('[autofocus]') || dialog.querySelector('input,select') || dialog.querySelector('[data-action="close-dialog"]'))?.focus());
}
function closeDialog() {dialog.close();}
dialog.addEventListener('close', () => { dialogTrigger?.focus(); dialogTrigger = null; });
dialog.addEventListener('click', event => {if (event.target === dialog) {const rect = dialog.getBoundingClientRect(); if(event.clientX < rect.left || event.clientX > rect.right || event.clientY < rect.top || event.clientY > rect.bottom) closeDialog();}});
const actions = (primary, danger = false) => `<div class="dialog-actions"><button class="button" type="button" data-action="close-dialog">Cancel</button><button class="button ${danger ? 'danger' : 'primary'}" type="submit">${primary}</button></div>`;
function formError(form, message, inputName) {
 let error = form.querySelector('.form-error'); if (!error) {error = document.createElement('p'); error.className = 'form-error'; error.setAttribute('role','alert'); form.querySelector('.dialog-actions').before(error);}
 error.textContent = message;
 const field = form.elements[inputName]; if(field) {field.setAttribute('aria-invalid','true'); field.focus();}
}
function mobileFooter() {return `<div class="mobile-only mobile-reset"><span>Prototype · Sample data</span><button class="text-button" data-action="reset">Reset prototype</button></div>`;}
function render(focus = false) {
 closeMenu();
 let route = activeRoute();
 if (route === '/signin') { renderSignIn(); return; }
 if (route.startsWith('/settings') && currentUser.role !== 'Administrator') { location.hash = '/files'; return; }
 const section = route.split('/')[1];
 const title = ({files:'Files',trash:'Trash',account:'Account',settings:'Settings'})[section] || 'Files';
 document.title = `${instanceName} · ${title}`;
 app.innerHTML = `<header class="header">${brand()}<div class="header-actions"><span class="prototype-tag">Prototype</span>${themeButton()}</div></header><div class="layout"><aside class="sidebar" aria-label="Main navigation"><nav class="nav-group" aria-label="Files"><a class="nav-link ${section==='files'?'active':''}" ${section==='files'?'aria-current="page"':''} href="#/files">${svg('folder')}<span>Files</span></a><a class="nav-link ${section==='trash'?'active':''}" ${section==='trash'?'aria-current="page"':''} href="#/trash">${svg('trash')}<span>Trash</span></a></nav><div class="nav-bottom"><nav class="nav-group" aria-label="Personal and administration">${currentUser.role==='Administrator'?`<a class="nav-link ${section==='settings'?'active':''}" ${section==='settings'?'aria-current="page"':''} href="#/settings/accounts" title="Settings" aria-label="Settings">${svg('settings')}<span>Settings</span></a>`:''}<a class="nav-link ${section==='account'?'active':''}" ${section==='account'?'aria-current="page"':''} href="#/account" title="Account" aria-label="Account">${svg('user')}<span>Account</span></a></nav><div class="prototype-footer"><span>Sample data</span><button class="text-button" data-action="reset">Reset</button></div></div></aside><main class="main" id="main" tabindex="-1">${section==='trash'?trashView():section==='account'?accountView():section==='settings'?settingsView(route):filesView(route)}${mobileFooter()}</main></div>`;
 const picker = app.querySelector('#file-picker');
 picker?.addEventListener('change', () => {chosenFile = picker.files[0] || null; render(); if(chosenFile) app.querySelector('[data-action="upload"]')?.focus();});
 app.querySelector('#instance-form')?.addEventListener('submit', saveInstance);
 if (focus) {const heading = app.querySelector('h1'); heading?.setAttribute('tabindex','-1'); heading?.focus({preventScroll:true});}
}
function filesView(route) {
 const id = decodeURIComponent(route.split('/')[2] || '');
 let folder = id ? files.find(f => f.id === id && f.type === 'folder' && !f.trashed) : null;
 if(id && !folder) return `<div class="page-top"><h1>Folder unavailable</h1></div><p class="muted">This folder is no longer available in the demo.</p><p style="margin-top:24px"><a class="button" href="#/files">Back to Files</a></p>`;
 const parent = folder?.id || null;
 const rows = files.filter(f => f.parent === parent && !f.trashed).sort((a,b) => (a.type===b.type?0:a.type==='folder'?-1:1)||a.name.localeCompare(b.name));
 const crumbs = folder ? `<nav class="breadcrumb" aria-label="Breadcrumb"><a href="#/files">Files</a>${pathOf(folder.id).map((f,i,arr)=>`${svg('chevron')}${i===arr.length-1?`<span aria-current="page">${esc(f.name)}</span>`:`<a href="#/files/${f.id}">${esc(f.name)}</a>`}`).join('')}</nav>`:'';
 return `${crumbs}<div class="page-top"><div class="page-title"><h1>${folder?esc(folder.name):'Files'}</h1></div></div><div class="toolbar" aria-label="File actions"><button class="button" data-action="new-folder" data-parent="${parent||''}">${svg('plus')}New folder</button><input class="file-input" type="file" id="file-picker" tabindex="-1" aria-label="Choose a file for simulated upload"><button class="button" data-action="choose-file">${svg('file')}Choose file</button><button class="button primary" data-action="upload" data-parent="${parent||''}" ${!chosenFile?'disabled':''}>${svg('upload')}Upload</button>${chosenFile?`<span class="chosen-file" title="${esc(chosenFile.name)}">${esc(chosenFile.name)}</span>`:''}</div><div class="listing"><div class="list-head"><span>Name</span><span class="size">Size</span><span></span></div>${rows.length?rows.map(file => fileRow(file)).join(''):`<div class="empty">${svg('folder')}<h2>No files.</h2></div>`}</div><p class="list-summary">${rows.length} ${rows.length===1?'item':'items'}</p>`;
}
function fileRow(file, trash = false) {
 return `<div class="file-row"><button class="file-name ${file.type==='folder'?'folder':''}" data-action="${trash?'trash-details':'open'}" data-id="${file.id}" title="${esc(file.name)}">${svg(fileIcon(file))}<span class="name">${esc(file.name)}</span></button><span class="file-size">${file.type==='folder'?'—':formatSize(file.size||0)}</span><button class="icon-button" data-action="${trash?'trash-menu':'file-menu'}" data-id="${file.id}" aria-label="Actions for ${esc(file.name)}" aria-haspopup="menu" aria-expanded="false">${svg('more')}</button></div>`;
}
function trashView() {
 const rows = files.filter(f => f.trashed && !files.find(p => p.id===f.parent)?.trashed);
 return `<div class="page-top"><div class="page-title"><h1>Trash</h1><p>Restore files from Trash.</p></div></div><div class="listing"><div class="list-head"><span>Name</span><span class="size">Size</span><span></span></div>${rows.length?rows.map(f=>fileRow(f,true)).join(''):`<div class="empty">${svg('trash')}<h2>No files.</h2><a class="button" href="#/files">Back to Files</a></div>`}</div><p class="list-summary">${rows.length} ${rows.length===1?'item':'items'}</p>`;
}
function accountView() {
 return `<div class="account-content"><div class="page-top"><h1>Account</h1></div><section class="section"><div class="account-profile"><div class="avatar" aria-hidden="true">${currentUser.name.split(' ').map(n=>n[0]).join('')}</div><div><p class="profile-name">${esc(currentUser.name)}</p><p class="profile-role">${esc(currentUser.role)}</p></div></div><dl class="details"><dt>Username</dt><dd>${esc(currentUser.username)}</dd><dt>Role</dt><dd>${esc(currentUser.role)}</dd></dl></section><section class="section"><div class="section-heading"><div><h2>Password</h2><p>Change the password for your account.</p></div><button class="button" data-action="password">Change password</button></div><p class="account-note">Prototype only. No real password is requested or changed.</p></section><section class="section"><div class="section-heading"><div><h2>Session</h2><p>Return to the sign-in screen.</p></div><button class="button" data-action="signout">${svg('logout')}Sign out</button></div></section></div>`;
}
function settingsView(route) {
 const instance = route.endsWith('/instance');
 return `<div class="settings-content"><div class="page-top"><div class="page-title"><h1>Settings</h1><p>Manage this instance and its accounts.</p></div></div><nav class="settings-tabs" aria-label="Settings"><a href="#/settings/accounts" class="${!instance?'active':''}" ${!instance?'aria-current="page"':''}>Accounts</a><a href="#/settings/instance" class="${instance?'active':''}" ${instance?'aria-current="page"':''}>Instance</a></nav>${instance?`<section class="section"><div class="section-heading"><div><h2>Instance name</h2><p>Shown to everyone in the header and on the sign-in screen.</p></div></div><form id="instance-form"><div class="form-field"><label for="instance-name">Name</label><input id="instance-name" name="instanceName" value="${esc(instanceName)}" maxlength="40" required autocomplete="off"><small>1–40 characters.</small></div><button class="button primary" type="submit">Save name</button><div id="instance-feedback" aria-live="polite"></div></form><p class="account-note">Saved only in this browser for the prototype.</p></section>`:`<section class="section"><div class="section-heading"><div><h2>Accounts</h2><p>Each account has its own private files.</p></div><button class="button" data-action="create-account">${svg('plus')}Create account</button></div><div class="account-list">${accounts.map(a=>`<div class="account-row"><div><p>${esc(a.name)}${a.id===currentUser.id?' <span class="muted">(you)</span>':''}</p><small>${esc(a.username)}<span class="mobile-role"> · ${a.role}</span></small></div><span class="role">${a.role}</span><span><span class="badge ${a.enabled?'enabled':''}">${a.enabled?'Enabled':'Disabled'}</span></span><button class="icon-button" data-action="account-menu" data-id="${a.id}" aria-label="Actions for ${esc(a.name)}" aria-haspopup="menu" aria-expanded="false">${svg('more')}</button></div>`).join('')}</div><p class="account-note">Account actions are simulated. No real accounts or passwords are changed.</p></section>`}</div>`;
}
function renderSignIn() {
 document.title = `${instanceName} · Sign in`;
 app.innerHTML = `<div class="signin-page"><header class="signin-header">${brand()}${themeButton()}</header><main class="signin-main" id="main"><form class="signin-form" id="signin-form"><h1>Sign in to ${esc(instanceName)}</h1><p class="intro">Self-hosted file storage.</p><div class="form-field"><label for="signin-account">Account name</label><input id="signin-account" name="accountName" value="alex" autocomplete="off" autocapitalize="none" spellcheck="false" readonly></div><div class="form-field"><label for="signin-password">Password</label><input id="signin-password" name="samplePassword" type="password" value="alex-demo-only" autocomplete="off" readonly aria-describedby="signin-note"></div><button class="button primary" type="submit">Sign in</button><p class="demo-note" id="signin-note">Sample credentials. No real sign-in.</p><div class="demo-persona"><label for="demo-identity">Prototype account</label><select id="demo-identity" name="identity"><option value="alex">Alex Morgan · Administrator</option><option value="jamie" ${accounts.find(a=>a.id==='jamie')?.enabled?'':'disabled'}>Jamie Chen · User${accounts.find(a=>a.id==='jamie')?.enabled?'':' (disabled)'}</option></select></div></form></main><footer class="signin-footer">Prototype · <button class="text-button" data-action="reset">Reset sample data</button></footer></div>`;
 app.querySelector('#demo-identity').addEventListener('change',event => {
  const identity=event.target.value;
  app.querySelector('#signin-account').value=identity;
  app.querySelector('#signin-password').value=`${identity}-demo-only`;
 });
 app.querySelector('#signin-form').addEventListener('submit',event => {event.preventDefault(); const id = new FormData(event.target).get('identity'); currentUser=accounts.find(a=>a.id===id)||initialAccounts().find(a=>a.id===id); location.hash='/files';});
}
function saveInstance(event) {
 event.preventDefault(); const input=event.target.elements.instanceName; const name=input.value.trim(); const feedback=app.querySelector('#instance-feedback');
 if(!name || name.length>40) {input.setAttribute('aria-invalid','true'); feedback.innerHTML='<p class="form-error" role="alert" style="margin-top:14px">Enter a name between 1 and 40 characters.</p>';input.focus();return;}
 instanceName=name;try{localStorage.setItem('sylux-prototype-name',name);}catch{}
 render(); app.querySelector('#instance-feedback').innerHTML=`<p class="form-success">${svg('check')}Instance name saved.</p>`; app.querySelector('#instance-name').focus();
}
function itemMenu(trigger, items) {
 if (menuTrigger === trigger) { closeMenu(true); return; }
 closeMenu(); menuTrigger=trigger; trigger.setAttribute('aria-expanded','true');
 const rect=trigger.getBoundingClientRect();
 menuRoot.innerHTML=`<div class="menu" role="menu" aria-label="Actions">${items.map(item=>item.separator?'<div class="menu-separator" role="separator"></div>':`<button role="menuitem" class="${item.danger?'danger':''}" data-action="${item.action}" ${item.id?`data-id="${item.id}"`:''}>${svg(item.icon)}${item.label}</button>`).join('')}</div>`;
 const menu=menuRoot.firstElementChild; const width=menu.offsetWidth; const height=menu.offsetHeight;
 menu.style.left=`${Math.max(8,Math.min(rect.right-width,innerWidth-width-8))}px`;
 menu.style.top=`${rect.bottom+height+8>innerHeight?Math.max(8,rect.top-height-4):rect.bottom+4}px`;
 menu.querySelector('button')?.focus();
}
function preview(file) {
 const content = file.content ? `<pre class="preview-text">${esc(file.content)}</pre>` : `<div class="unsupported">${svg(fileIcon(file))}<h3>No preview available</h3><p>This file type can be downloaded to open.</p><button class="button" data-action="download" data-id="${file.id}">${svg('download')}Download</button><p class="dialog-note">Simulated download. No file will be saved.</p></div>`;
 showDialog(file.name,`<div class="preview-meta"><span>${formatSize(file.size||0)}</span><span>Sample file</span></div>${content}${file.content?`<div class="dialog-actions"><button class="button" data-action="download" data-id="${file.id}">${svg('download')}Download</button></div><p class="dialog-note">Preview uses synthetic text. Downloads are simulated.</p>`:''}`,null,'preview-dialog');
}
function nameDialog(type, item, parent=null) {
 const rename=type==='rename';
 showDialog(rename?'Rename item':'New folder',`<form><div class="form-field"><label for="item-name">${rename?'Name':'Folder name'}</label><input id="item-name" name="name" value="${rename?esc(item.name):''}" placeholder="${rename?'':'Untitled folder'}" maxlength="120" autocomplete="off" required autofocus></div>${actions(rename?'Save name':'Create folder')}</form>`,(data,form)=>{
  const name=String(data.get('name')).trim();
  if(!name || /[/\\]/.test(name)) {formError(form,'Enter a name without slashes.','name');return;}
  const targetParent=rename?item.parent:parent;
  if(files.some(f=>!f.trashed&&f.parent===targetParent&&f.name.toLowerCase()===name.toLowerCase()&&f.id!==item?.id)) {formError(form,'An item with this name already exists. Choose another name.','name');return;}
  if(rename) item.name=name; else files.push({id:`folder-${Date.now()}`,name,type:'folder',parent});
  closeDialog(); render(true); notify(rename?'Item renamed.':'Folder created.');
 });
 if(rename) dialog.querySelector('input').select();
}
function moveDialog(file) {
 const descendants = new Set([file.id]); let changed=true;
 while(changed) {changed=false;for(const f of files)if(descendants.has(f.parent)&&!descendants.has(f.id)){descendants.add(f.id);changed=true;}}
 const folders=files.filter(f=>f.type==='folder'&&!f.trashed&&!descendants.has(f.id));
 showDialog('Move item',`<p>Choose a destination for <strong>${esc(file.name)}</strong>.</p><form><div class="form-field"><label for="destination">Destination</label><select name="destination" id="destination"><option value="" ${file.parent===null?'selected':''}>Files</option>${folders.map(f=>`<option value="${f.id}" ${file.parent===f.id?'selected':''}>Files / ${esc(pathOf(f.id).map(x=>x.name).join(' / '))}</option>`).join('')}</select></div>${actions('Move here')}</form>`,(data,form)=>{
  const parent=data.get('destination')||null;
  if(parent===file.parent){formError(form,'Choose a different folder.','destination');return;}
  if(files.some(f=>f.id!==file.id&&!f.trashed&&f.parent===parent&&f.name.toLowerCase()===file.name.toLowerCase())){formError(form,'An item with this name exists in that folder.','destination');return;}
  file.parent=parent;closeDialog();render(true);notify(`Moved ${file.name}.`);
 });
}
function trashDialog(file) {
 showDialog('Move to Trash?',`<p><strong>${esc(file.name)}</strong> will be moved to Trash. You can restore it later.</p><form>${actions('Move to Trash',true)}</form>`,()=>{file.trashed=true;closeDialog();render(true);notify('Item moved to Trash.');});
}
function restore(file) {
 const previous=files.find(f=>f.id===file.parent); const parent=previous&&!previous.trashed?previous.id:null;
 if(files.some(f=>f.id!==file.id&&!f.trashed&&f.parent===parent&&f.name===file.name)) {notify('Restore blocked: an item with this name already exists. Rename the existing item first.');return;}
 file.parent=parent;file.trashed=false;render(true);notify(`Restored ${file.name}${parent?' to '+previous.name:' to Files'}.`);
}

function createAccount() {
 showDialog('Create account',`<p>Create a simulated account. No invitation will be sent and no password is needed.</p><form><div class="form-field"><label for="display-name">Display name</label><input id="display-name" name="displayName" maxlength="60" autocomplete="off" required autofocus></div><div class="form-field"><label for="username">Username</label><input id="username" name="username" maxlength="32" autocomplete="off" autocapitalize="none" spellcheck="false" required><small>Letters, numbers, dots, hyphens and underscores.</small></div><div class="form-field"><label for="role">Role</label><select id="role" name="role"><option>User</option><option>Administrator</option></select></div>${actions('Create demo account')}</form>`,(data,form)=>{
 const name=String(data.get('displayName')).trim(); const username=String(data.get('username')).trim();
 if(!name){formError(form,'Enter a display name.','displayName');return;}
 if(!/^[a-zA-Z0-9._-]{1,32}$/.test(username)){formError(form,'Use letters, numbers, dots, hyphens or underscores.','username');return;}
 if(accounts.some(a=>a.username.toLowerCase()===username.toLowerCase())){formError(form,'This username is already in use. Choose another.','username');return;}
 accounts.push({id:`account-${Date.now()}`,name,username,role:data.get('role'),enabled:true});closeDialog();render(true);notify('Demo account created.');
 });
}
function passwordDialog() {
 showDialog('Change password',`<p>This is a prototype interaction. No real password is requested, stored or changed.</p><form><div class="form-field"><label for="demo-password">Example password</label><input id="demo-password" value="example-only-2026" readonly aria-describedby="password-demo-note"><small id="password-demo-note">A fixed example is used for this demo.</small></div>${actions('Simulate password change')}</form>`,()=>{closeDialog();notify('Password change simulated. No real password changed.');});
}
function accountReset(account) {
 showDialog('Reset password?',`<p>In Sylux, resetting <strong>${esc(account.name)}’s</strong> password would replace their current password. They would need the new password to sign in.</p><p>This prototype does not change or generate a real password.</p><form>${actions('Simulate reset')}</form>`,()=>{closeDialog();notify(`Password reset simulated for ${account.name}.`);});
}
function accountToggle(account) {
 const enable=!account.enabled;
 showDialog(`${enable?'Enable':'Disable'} account?`,`<p>${enable?`<strong>${esc(account.name)}</strong> would be able to sign in again.`:`<strong>${esc(account.name)}</strong> would no longer be able to sign in. Their files would remain stored.`}</p><p>This change affects sample data only.</p><form>${actions(enable?'Enable demo account':'Disable demo account',!enable)}</form>`,()=>{account.enabled=enable;closeDialog();render(true);notify(`Demo account ${enable?'enabled':'disabled'}.`);});
}
function resetPrototype() {
 showDialog('Reset prototype?',`<p>Restore the sample files and accounts, the Sylux instance name, and the system theme. Changes made in this demo will be cleared.</p><form>${actions('Reset prototype')}</form>`,()=>{
 files=sample();accounts=initialAccounts();currentUser=accounts[0];chosenFile=null;instanceName='Sylux';delete document.documentElement.dataset.theme;
 try{localStorage.removeItem('sylux-prototype-name');localStorage.removeItem('sylux-prototype-theme');}catch{}
 closeDialog();if(activeRoute()==='/files')render();else location.hash='/files';notify('Prototype reset.');
 });
}
document.addEventListener('click',event=>{
 if(event.target.closest('.skip-link')){event.preventDefault();document.querySelector('#main')?.focus();return;}
 const trigger=event.target.closest('[data-action]');
 if(!trigger){if(!event.target.closest('.menu'))closeMenu();return;}
 const action=trigger.dataset.action; const file=files.find(f=>f.id===trigger.dataset.id); const account=accounts.find(a=>a.id===trigger.dataset.id);
 if(!['file-menu','trash-menu','account-menu'].includes(action))closeMenu(true);
 switch(action){
 case'theme':{const theme=isDark()?'light':'dark';document.documentElement.dataset.theme=theme;try{localStorage.setItem('sylux-prototype-theme',theme);}catch{}render();document.querySelector('[data-action="theme"]')?.focus();break;}
 case'close-dialog':closeDialog();break;
 case'signout':location.hash='/signin';break;
 case'reset':resetPrototype();break;
 case'choose-file':app.querySelector('#file-picker')?.click();break;
 case'upload':{if(!chosenFile)break;const parent=trigger.dataset.parent||null;const selected=chosenFile;
 const perform=()=>{files.push({id:`upload-${Date.now()}`,name:selected.name,type:'file',size:selected.size,parent});chosenFile=null;render(true);notify('Upload simulated. Only the filename and size were used.');};
 if(files.some(f=>!f.trashed&&f.parent===parent&&f.name===selected.name))notify('An item with this name already exists. Choose a file with a different name for this demo.');else perform();break;}
 case'new-folder':nameDialog('create',null,trigger.dataset.parent||null);break;
 case'open':if(file?.type==='folder')location.hash=`/files/${file.id}`;else if(file)preview(file);break;
 case'file-menu':if(file)itemMenu(trigger,[{label:'Open',icon:'eye',action:'open',id:file.id},{label:'Rename',icon:'edit',action:'rename',id:file.id},{label:'Move',icon:'move',action:'move',id:file.id},...(file.type!=='folder'?[{label:'Download',icon:'download',action:'download',id:file.id}]:[]),...(file.type!=='folder'||!files.some(child=>child.parent===file.id&&!child.trashed)?[{separator:true},{label:'Move to Trash',icon:'trash',action:'trash',id:file.id,danger:true}]:[])]);break;
 case'trash-menu':if(file)itemMenu(trigger,[{label:'Restore',icon:'restore',action:'restore',id:file.id}]);break;
 case'trash-details':if(file)showDialog(file.name,`<p>Original location: <strong>Files${file.parent?' / '+esc(pathOf(file.parent).map(f=>f.name).join(' / ')):''}</strong></p><p>This item is in Trash. Restore it to use it again.</p><div class="dialog-actions"><button class="button" data-action="close-dialog">Close</button><button class="button primary" data-action="restore" data-id="${file.id}">${svg('restore')}Restore</button></div>`);break;
 case'rename':if(file)nameDialog('rename',file);break;
 case'move':if(file)moveDialog(file);break;
 case'trash':if(file)trashDialog(file);break;
 case'restore':if(file){if(dialog.open)closeDialog();restore(file);}break;
  case'download':if(file)notify(`Download simulated for ${file.name}. No file was saved.`);break;
 case'password':passwordDialog();break;
 case'create-account':createAccount();break;
 case'account-menu':if(account)itemMenu(trigger,[{label:'Reset password',icon:'key',action:'account-reset',id:account.id},...(account.id!==currentUser.id?[{label:account.enabled?'Disable account':'Enable account',icon:account.enabled?'user':'check',action:'account-toggle',id:account.id,danger:account.enabled}]:[])]);break;
 case'account-reset':if(account)accountReset(account);break;
 case'account-toggle':if(account)accountToggle(account);break;
 }
});
document.addEventListener('keydown',event=>{
 const menu=menuRoot.querySelector('.menu');
 if(!menu)return;
 const buttons=[...menu.querySelectorAll('button')]; const i=buttons.indexOf(document.activeElement);
 if(event.key==='Escape'){event.preventDefault();closeMenu(true);}
 else if(event.key==='ArrowDown'||event.key==='ArrowUp'){event.preventDefault();buttons[(i+(event.key==='ArrowDown'?1:-1)+buttons.length)%buttons.length]?.focus();}
 else if(event.key==='Home'){event.preventDefault();buttons[0]?.focus();}
 else if(event.key==='End'){event.preventDefault();buttons.at(-1)?.focus();}
 else if(event.key==='Tab')closeMenu();
});
window.addEventListener('hashchange',()=>{if(dialog.open)closeDialog();chosenFile=null;render(true);window.scrollTo(0,0);});
window.addEventListener('resize',()=>closeMenu());
window.addEventListener('scroll',()=>closeMenu(),true);
matchMedia('(prefers-color-scheme: dark)').addEventListener('change',()=>{if(!document.documentElement.dataset.theme)render();});
render();
