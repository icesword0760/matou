/* Local design prototype. All messages, accounts, quotas and device states are synthetic. */
const paths={
 search:'<circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/>',
 plus:'<path d="M12 5v14M5 12h14"/>',
 chevron:'<path d="m9 5 7 7-7 7"/>',down:'<path d="m6 9 6 6 6-6"/>',back:'<path d="m14 5-7 7 7 7"/>',
 chat:'<path d="M5 4h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H9l-5 3V6a2 2 0 0 1 1-2Z"/><path d="M8 9h9M8 13h6"/>',
 account:'<circle cx="12" cy="8" r="3.4"/><path d="M5 21v-2a7 7 0 0 1 14 0v2"/>',
 computer:'<rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/>',
 bell:'<path d="M5 17h14l-2-3V9a5 5 0 0 0-10 0v5l-2 3ZM10 21h4"/>',
 folder:'<path d="M3 7a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>',
 branch:'<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="5" r="2"/><path d="M6 7v10M18 7c0 7-12 3-12 10"/>',
 dots:'<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
 terminal:'<path d="m4 6 6 6-6 6M13 18h7"/>',
 check:'<path d="m5 12 4 4L19 6"/>',doublecheck:'<path d="m2 12 4 4L16 6M10 15l2 2L22 7"/>',
 arrow:'<path d="M12 19V5m-6 6 6-6 6 6"/>',
 file:'<path d="M13 3H5v18h14V9l-6-6Zm0 0v6h6M8 13h8M8 17h5"/>',
 clock:'<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
 refresh:'<path d="M20 8a8 8 0 0 0-14-3L3 8m0-5v5h5M4 16a8 8 0 0 0 14 3l3-3m0 5v-5h-5"/>',
 info:'<circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/>',
 shield:'<path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z"/><path d="m8 12 3 3 5-6"/>',
 close:'<path d="m6 6 12 12M6 18 18 6"/>',
 copy:'<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M16 8V3H3v13h5"/>',
 stop:'<rect x="6" y="6" width="12" height="12" rx="2"/>',
 signal:'<path d="M4 19v-3M9 19v-7M14 19V8M19 19V4"/>',
 wifi:'<path d="M2 8a16 16 0 0 1 20 0M5 12a11 11 0 0 1 14 0M8 16a6 6 0 0 1 8 0M12 20h.01"/>',
 switch:'<path d="M4 7h16m-4-4 4 4-4 4M20 17H4m4-4-4 4 4 4"/>',
 grid:'<rect x="4" y="4" width="6" height="6" rx="1"/><rect x="14" y="4" width="6" height="6" rx="1"/><rect x="4" y="14" width="6" height="6" rx="1"/><rect x="14" y="14" width="6" height="6" rx="1"/>',
 scan:'<path d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5M4 12h16"/>',
 key:'<circle cx="8" cy="9" r="5"/><path d="m12 13 8 8m-3-3 3-3"/>'
};
const icon=(name,cls='')=>`<svg aria-hidden="true" class="${cls}" viewBox="0 0 24 24">${paths[name]||paths.chat}</svg>`;
const escapeHTML=s=>String(s).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const button=(action,content,cls='',label='')=>`<button type="button" data-action="${action}" class="${cls}"${label?` aria-label="${label}"`:''}>${content}</button>`;
const sessions=[
 {title:'支付回调 · 回归测试',project:'shop-api',task:'支付回调幂等性',canvas:'实现与验证',preview:'重复回调应返回成功，还是拒绝？需要你确认后继续。',time:'刚刚',state:'wait',stateText:'等你回复',meta:'1 个待确认',icon:'chat'},
 {title:'移动端 · 页面设计',project:'Matou',task:'移动端探索',canvas:'设计',preview:'会话列表、对话和账户页已整理好，可以继续细化。',time:'2 分钟前',state:'done',stateText:'已完成',meta:'Claude Code',icon:'chat'},
 {title:'实现 · Redis 幂等键',project:'shop-api',task:'支付回调幂等性',canvas:'实现与验证',preview:'正在补充边界用例，随后运行支付模块测试。',time:'5 分钟前',state:'running',stateText:'运行中',meta:'Claude Code',icon:'terminal'},
 {title:'审查 · 登录流程',project:'Matou',task:'登录体验优化',canvas:'审查',preview:'已整理 3 个可以优化的地方，等你继续。',time:'18 分钟前',state:'done',stateText:'已完成',meta:'Claude Code',icon:'chat'}
];
const states=[0,1,2].map((i)=>({view:['sessions','chat','accounts'][i],filter:'all',project:'全部工作空间',query:'',session:1,tab:'dialogue',sheet:null,draft:'',messages:[],replying:false,permission:false,decision:'',toast:'',remember:false}));
let currentAccount='work',online=true,switched=false,rememberScope=false,refreshed=false;
const accountName=()=>currentAccount==='work'?'工作账户':'个人账户';
function statusbar(){return `<div class="statusbar"><span>9:41</span><span class="system-icons">${icon('signal')}${icon('wifi')}<span class="battery"><i></i></span></span></div>`;}
function nav(s){return `<nav class="nav-bar" aria-label="主要导航">${[['sessions','chat','会话'],['accounts','account','账户'],['device','computer','设备']].map(([page,ic,label])=>button(`view:${page}`,`<span class="nav-icon">${icon(ic)}${page==='sessions'?'<i class="notify-dot"></i>':''}</span>${label}`,s.view===page?'active':'')).join('')}</nav>`;}
function deviceStrip(){return button('view:device',`${icon('computer')}<span>家里的 Mac</span><span class="end"><i class="dot"></i>${online?'在线':'已离线'}</span>${icon('chevron','chevron')}`,`device-strip ${online?'':'offline'}`);}
function heading(title,subtitle,action=''){return `<header class="app-header"><div><h2>${title}</h2><div class="subhead">${subtitle}</div></div><div class="right">${action}</div></header>`;}
function sessionRows(s){
 const filtered=sessions.map((v,i)=>({...v,index:i})).filter(v=>(s.filter==='all'||v.state===s.filter)&&(s.project==='全部工作空间'||v.project===s.project)&&`${v.title}${v.project}${v.preview}`.toLowerCase().includes(s.query.toLowerCase()));
 if(!filtered.length)return '<p class="empty-result">没有找到匹配的会话<br>试试其他名称或工作空间</p>';
 return filtered.map((v,i)=>`${i===0?`<div class="section-label">${v.state==='wait'?'<span class="attention">●</span> 需要你处理':'最近会话'}<span class="count">${v.state==='wait'?'1':'今天'}</span></div>`:''}${v.state!=='wait'&&i>0&&filtered[i-1].state==='wait'?'<div class="section-label">最近会话<span class="count">今天</span></div>':''}${button(`session:${v.index}`,`<div class="session-top">${icon(v.icon)}<h3>${v.title}</h3><time>${v.time}</time></div><div class="session-path">${v.project}<span> / </span>${v.task}</div><p class="session-preview">${v.preview}</p><div class="session-foot"><span class="status-tag ${v.state}"><i class="dot"></i>${v.stateText}</span><span>${v.meta}</span>${v.index===1?'<span class="unread">2</span>':''}</div>`,`session ${v.state==='wait'?'needs':''}`)}`).join('');
}
function home(s){return `${heading('会话','随时接上你的工作',button('sheet:notifications',icon('bell'),'icon-button','通知')+button('sheet:new',icon('plus'),'icon-button blue','新建会话'))}${deviceStrip()}${!online?'<div class="offline-notice">电脑暂时离线。可查看已同步的对话，恢复连接后再发送。</div>':''}<div class="page-scroll"><label class="search">${icon('search')}<input aria-label="搜索会话" data-search placeholder="搜索会话、工作空间" value="${escapeHTML(s.query)}"></label><div class="workspace-filter">${button('sheet:workspaces',s.project+icon('down'))}<span>4 个会话</span></div><div class="filters">${[['all','全部','4'],['wait','待处理','1'],['running','运行中','1']].map(([id,t,c])=>button('filter:'+id,t+`<b>${c}</b>`,s.filter===id?'active':'')).join('')}</div><div class="session-results">${sessionRows(s)}</div></div>${nav(s)}`;}
function conversation(s){
 const v=sessions[s.session];
 const text=s.session===1?`三个核心页面已经整理好了。移动端以<strong>继续现有会话</strong>为主，首页优先显示需要你处理的消息。`:s.session===0?'重复回调目前返回 409，现有实现准备改为 200，并带上 duplicate 标记。需要你确认期望行为。':s.session===2?'幂等键校验已经加到入口，正在补充边界测试，完成后我会把结果整理给你。':'登录流程已完成审查。下面三个位置可以继续改善：错误提示、授权返回和会话恢复。';
 const userText=s.session===1?'先出一版手机上的会话页，延续桌面端的浅色风格。':'接着电脑上的进度，告诉我现在做到哪里了。';
 let transcript=`<div class="date-line">今天 09:36 · 从电脑接续</div><div class="user-bubble">${userText}</div><div class="message-meta">电脑发送 ${icon('doublecheck')}</div><div class="assistant-name"><span class="agent-mark">✳</span><strong>Claude Code</strong><time>09:40</time></div><div class="assistant-text">${text}</div><div class="execution">${button('toggle:execution',`${icon('check')}<span>已查看 3 个文件，完成页面草图</span>${icon('down','arrow')}`)}<pre hidden>Read · 会话列表与账户面板\nRead · 当前账户切换行为\nWrite · 移动端页面草图\n以上为设计示例</pre></div>`;
 if(s.session===1)transcript+=`<div class="change-summary"><div class="change-top">${icon('file')}<span>2 个文件变更</span><span class="diff"><span class="plus">+128</span><span class="minus">−12</span></span></div><div class="file-item"><code>MobileSession.tsx</code><span>会话页面</span></div><div class="file-item"><code>mobile.css</code><span>页面样式</span></div></div><div class="assistant-text">账户入口放在底部；在对话里也可以直接查看用量和切换账户。<br><strong>下一步要不要细化账户切换的交互？</strong></div>`;
 if((s.permission||s.session===0)&&!s.decision)transcript+=`<div class="permission-card"><strong>需要你确认</strong><p>${s.permission?'允许在家里的 Mac 运行以下测试？<br><code>pnpm test:unit</code>':'将重复支付回调的预期状态码改为 200，再继续运行测试。'}</p><div class="permission-actions">${button('permission:deny','暂不执行')}${button('permission:allow',s.permission?'允许这一次':'确认并继续','allow')}</div></div>`;
 if(s.decision)transcript+=`<div class="success-banner" style="margin:14px 0">${icon('check')}${escapeHTML(s.decision)}</div>`;
 transcript+=`<div class="assistant-actions">${icon('check')} 已同步到手机 <span>·</span> ${icon('clock')} 1 分钟前</div>`;
 if(!s.messages.length)transcript+=`<div class="suggestions">${button('suggest:继续细化账户切换的交互。','继续细化账户切换')}${button('suggest:先给我看一下当前改动。','查看当前改动')}</div>`;
 for(const m of s.messages)transcript+=m.kind==='user'?`<div class="user-bubble" style="margin-top:18px">${escapeHTML(m.text)}</div><div class="message-meta">手机发送 · 示例 ${icon('doublecheck')}</div>`:`<div class="assistant-name"><span class="agent-mark">✳</span><strong>Claude Code</strong></div><div class="assistant-text">${escapeHTML(m.text)}</div>`;
 if(s.replying)transcript+='<div class="assistant-text" role="status">正在家里的 Mac 上处理…</div>';
 if(s.tab==='terminal')transcript=`<div class="date-line">原始输出 · 演示数据</div><pre class="terminal">~/projects/matou\n\n❯ ${userText}\n\n● Read MobileSession.tsx\n  Read 92 lines\n\n● Update mobile.css\n  +128 lines / −12 lines\n\n${text.replace(/<[^>]+>/g,'')}\n\n❯ </pre><div class="terminal-keys">${['Esc','Tab','↑','↓','Ctrl+C'].map(k=>button('key:'+k,k)).join('')}</div><div class="device-note">保留原始输出；较长内容可在终端区域内滚动查看。</div>`;
 return `<header class="app-header chat-header">${button('view:sessions',icon('back'),'icon-button','返回会话列表')}<div><h2>${v.title}</h2><div class="subhead">${v.project} / ${v.task} / ${v.canvas}</div></div><div class="right">${button('sheet:sessionmenu',icon('dots'),'icon-button','会话操作')}</div></header><div class="chat-context"><span>${icon('branch')} mobile-preview</span><span class="live"><i class="dot"></i>${online?'家里的 Mac · 已同步':'电脑离线 · 只读'}</span></div><div class="view-switch">${button('tab:dialogue',icon('chat')+'对话',s.tab==='dialogue'?'active':'')}${button('tab:terminal',icon('terminal')+'原始终端',s.tab==='terminal'?'active':'')}</div><div class="page-scroll chat-content">${transcript}</div><div class="compose-zone">${!online?'<p class="compose-offline">连接已断开，草稿保留在手机上。恢复连接后由你发送。</p>':''}<div class="account-shortcut">${button('view:accounts',icon('account')+accountName()+icon('down'))}<span class="quota-chip">5h 已用 ${currentAccount==='work'?'94':'22'}%</span><span>上下文 38%</span></div><form class="input-box"><textarea aria-label="继续对话" placeholder="接着聊，告诉它下一步…" maxlength="2000">${escapeHTML(s.draft)}</textarea><div class="input-tools"><span class="hint">${icon('computer')} 在家里的 Mac 上执行</span><button type="submit" class="send" aria-label="${s.replying?'停止示例回复':'发送消息'}" ${!online?'disabled':''}>${icon(s.replying?'stop':'arrow')}</button></div></form></div>`;
}
function quota(name,value,reset){return `<div class="quota"><div class="quota-title"><span>${name}已用</span><b class="${value>=90?'high':''}">${value}%</b></div><div class="meter ${value>=90?'high':''}"><i style="width:${value}%"></i></div><small>${reset}重置</small></div>`;}
function accounts(s){return `${heading('账户','管理家里电脑上的登录',button('sheet:addaccount',icon('plus'),'icon-button soft','添加账户'))}${deviceStrip()}<div class="page-scroll"><p class="account-intro">手机切换后，家里的电脑同步使用。<br>会话还在，随时可以继续。</p>${switched?`<div class="success-banner">${icon('check')}<div>已切换为${accountName()}。现有任务继续运行，账户生效可能有延迟。${button('sheet:restart','立即重启并续接…')}</div></div>`:''}${[['work','工作账户','工',94,71,'1 小时 26 分后','周一 09:00'],['personal','个人账户','个',22,36,'3 小时 12 分后','周四 09:00']].map(([id,name,letter,five,week,r1,r2])=>`<article class="account-card ${currentAccount===id?'current':''}"><div class="account-title"><span class="avatar ${id==='personal'?'personal':''}">${letter}</span><div><h3>${name}</h3><p>已在家里的 Mac 登录</p></div>${currentAccount===id?'<span class="current-pill">电脑默认</span>':button('switch:'+id,'切换','switch-account')}</div><div class="quota-grid">${quota('5 小时',five,r1)}${quota('本周',week,r2)}</div>${id==='work'?`<div class="usage-tip">${icon('info')}5 小时额度即将用完，可切换其他账户</div>`:''}</article>`).join('')}<div class="account-updated"><span>${refreshed?'刚刚更新':'1 分钟前更新'} · 示例用量</span>${button('refresh',icon('refresh')+'刷新')}</div><div class="info-block">${icon('info')}<div><strong>切换的是家里电脑的默认账户</strong>该电脑上共享默认登录的会话都会受影响，正在运行的任务不会自动重启。</div></div>${button('sheet:provider',`${icon('grid')}服务供应商<span class="row-end">Claude 官方</span>${icon('chevron','chevron')}`,'settings-row')}${button('sheet:addaccount',`${icon('plus')}添加其他账户${icon('chevron','chevron')}`,'settings-row')}</div>${nav(s)}`;}
function device(s){return `${heading('设备','让工作留在电脑上',button('sheet:pair',icon('scan'),'icon-button soft','连接电脑'))}<div class="page-scroll"><div class="device-hero"><div class="computer-art"><div class="monitor"><div class="line"></div><div class="line"></div><div class="line"></div><span class="online-dot" ${online?'':'style="background:#bc965e"'}></span></div></div><h3>家里的 Mac</h3><p>Mac mini · Matou 桌面端<br>${online?'这台电脑为你继续运行会话':'连接中断，最后在线 09:41'}</p><span class="device-status ${online?'':'offline'}"><i class="dot"></i>${online?'已连接 · 远程访问':'暂时离线'}</span></div><div class="device-stats"><div><strong>${online?'4':'—'}</strong><p>可继续的会话</p></div><div><strong>${online?'1':'—'}</strong><p>运行中</p></div><div><strong>${online?'刚刚':'09:41'}</strong><p>最近同步</p></div></div>${!online?'<div class="info-block">'+icon('info')+'<div><strong>电脑可能休眠，或网络中断</strong>已同步的对话可以继续查看。未发送的内容保留为草稿，连接恢复后由你决定发送。</div></div>':''}${button(online?'view:sessions':'reconnect',icon(online?'chat':'refresh')+(online?'回到会话':'重新连接'),'wide-primary')}${button('sheet:pair',`${icon('scan')}配对另一台电脑<span class="row-end">扫码连接</span>${icon('chevron','chevron')}`,'settings-row')}${button('sheet:notifysettings',`${icon('bell')}消息通知<span class="row-end">仅在需要我时</span>${icon('chevron','chevron')}`,'settings-row')}${button('sheet:security',`${icon('shield')}访问授权<span class="row-end">已授权此手机</span>${icon('chevron','chevron')}`,'settings-row')}<p class="device-note"><strong>出门前，保持电脑开机并联网。</strong><br>需要回复、操作确认、任务完成时，手机会收到通知。手机退出页面后，电脑上的任务仍继续运行。</p><div class="demo-toggle"><span>原型演示 · 连接状态</span>${button('toggle:offline',online?'模拟离线':'恢复在线')}</div></div>${nav(s)}`;}
function sheet(s){
 if(!s.sheet)return '';
 let title='',body='',actions='';
 const close=button('close','暂不切换');
 switch(s.sheet){
 case 'switch':title=`切换到${s.selected==='personal'?'个人账户':'工作账户'}？`;body='<p>这会更改「家里的 Mac」的默认登录。</p><div class="impact"><strong>影响该电脑上共享默认登录的会话</strong>包括 Matou 里的会话，以及其他终端和编辑器。使用 API Key 或自定义供应商的会话不在此范围。</div><p>现有任务继续运行，不会自动重启。切换成功后，可以再选择立即重启并续接。</p><label class="checkline"><input type="checkbox" data-remember>已了解范围，下次直接切换</label>';actions=close+button('confirm-switch','立即切换','primary');break;
 case 'restart':title='重启并接上原来的对话？';body='<p>将重启家里的 Mac 上，Matou 内全部正在运行的 Claude 会话。</p><div class="impact warn"><strong>进行中的任务会被打断</strong>重启后尝试接回各自的对话。普通 Shell 和已退出的会话不受影响。</div>';actions=button('close','暂不重启')+button('confirm-restart','重启并续接','primary');break;
 case 'workspaces':title='工作空间';body=['全部工作空间','Matou','shop-api'].map(t=>button('project:'+t,icon('folder')+t+(s.project===t?'<span>已选</span>':''),'option')).join('');break;
 case 'notifications':title='需要你看一眼';body=button('session:0',icon('chat')+'支付回调 · 等待你的确认'+icon('chevron'),'option')+button('session:1',icon('check')+'移动端页面设计已完成'+icon('chevron'),'option');break;
 case 'new':title='新建会话';body='<p>在家里电脑的工作空间中创建，并从手机开始对话。</p><label class="field" for="new-title">会话名称</label><input class="text-field" id="new-title" value="新的想法"><div class="impact" style="margin-top:14px"><strong>Matou / 移动端探索 / 设计</strong>沿用工作空间目录和电脑默认账户。</div>';actions=button('close','取消')+button('create-demo','开始对话','primary');break;
 case 'sessionmenu':title='当前会话';body=button('sheet:details',icon('info')+'会话信息','option')+button('view:accounts',icon('account')+'账户与用量','option')+button('permission-demo',icon('shield')+'预览操作确认','option')+button('sheet:interrupt',icon('stop')+'中断当前任务','option');break;
 case 'details':title='会话信息';body=`<div class="impact"><strong>${sessions[s.session].title}</strong>${sessions[s.session].project} / ${sessions[s.session].task} / ${sessions[s.session].canvas}<br>执行设备：家里的 Mac<br>默认账户：${accountName()}<br>运行身份：以电脑实际回传为准</div>`;break;
 case 'interrupt':title='中断当前任务？';body='<p>停止当前会话正在执行的任务，对话和已有输出仍保留。</p>';actions=button('close','继续运行')+button('interrupt','中断任务','primary');break;
 case 'provider':title='服务供应商';body='<p>当前使用 Claude 官方登录。自定义供应商会影响家里电脑的全局配置。</p>'+button('close',icon('check')+'Claude 官方<span>使用中</span>','option')+'<div class="impact" style="margin-top:15px">本轮优先设计已保存账户的远程切换，新增供应商沿用桌面端配置。</div>';break;
 case 'addaccount':title='添加其他账户';body='<p>首版建议在家里电脑上完成新账户登录，手机上选择已保存账户即可切换。</p><div class="impact"><strong>电脑上：账户 → 添加账户</strong>完成登录并保存后，该账户会同步到手机。手机端直接授权流程需另行设计。</div>';break;
 case 'pair':title='连接家里的电脑';body='<p>首次配对时，让手机与电脑都在你身边。</p><div class="impact"><strong>1. 电脑上开启远程访问</strong>在 Matou 的移动设备设置中显示配对码。<br><br><strong>2. 手机扫码并核对电脑名称</strong>在电脑上确认授权后，外出也能接续会话。</div><p>配对入口为移动端方案的一部分，尚未实现。</p>';break;
 case 'notifysettings':title='仅在需要你时通知';body='<div class="impact"><strong>需要回复 · 操作确认 · 任务完成 · 出错</strong>这四类通知直接带你回到对应会话。普通运行进度保持安静。</div><p>系统推送为方案示意，原型不会申请权限或发送通知。</p>';break;
 case 'security':title='已授权此手机';body='<div class="impact"><strong>你的 Android 手机</strong>允许查看会话、发送消息、切换已保存账户。</div><p>电脑端可以撤销手机的访问授权。账户登录信息保留在电脑，手机仅发起操作。</p>';break;
 }
 return `<div class="sheet-layer"><section class="sheet" role="dialog" aria-modal="true" aria-label="${title}" tabindex="-1"><div class="sheet-handle"></div><header class="sheet-header"><h3>${title}</h3>${button('close',icon('close'),'icon-button','关闭')}</header>${body}${actions?`<div class="sheet-actions">${actions}</div>`:button('close','知道了','plain-action')}</section></div>`;
}
function render(i){const s=states[i],el=document.querySelector('#phone-'+i);const html=s.view==='sessions'?home(s):s.view==='chat'?conversation(s):s.view==='accounts'?accounts(s):device(s);el.innerHTML=statusbar()+`<div class="app" ${s.sheet?'inert':''}>${html}</div>`+sheet(s)+(s.toast?`<div class="toast" role="status">${escapeHTML(s.toast)}</div>`:'');if(s.sheet)requestAnimationFrame(()=>el.querySelector('.sheet').focus({preventScroll:true}));}
function renderAll(){states.forEach((_,i)=>render(i));}
function toast(i,text){states[i].toast=text;render(i);setTimeout(()=>{if(states[i].toast===text){states[i].toast='';render(i);}},2700);}
function switchAccount(i){if(!online){states[i].sheet=null;toast(i,'电脑已离线，恢复连接后才能切换账户。');return;}currentAccount=states[i].selected;rememberScope=states[i].remember||rememberScope;switched=true;states[i].sheet=null;renderAll();}
function action(i,a){const s=states[i];const [name,...rest]=a.split(':');const value=rest.join(':');
 if(name==='view'){s.view=value;s.sheet=null;render(i);return;}
 if(name==='sheet'){s.sheet=value;s.remember=false;render(i);return;}
 if(name==='session'){s.session=Number(value);s.view='chat';s.sheet=null;s.messages=[];s.replying=false;s.permission=false;s.decision='';s.draft='';render(i);return;}
 if(name==='close'){s.sheet=null;render(i);return;}
 if(name==='filter'){s.filter=value;render(i);return;}
 if(name==='project'){s.project=value;s.sheet=null;render(i);return;}
 if(name==='tab'){s.tab=value;render(i);return;}
 if(name==='switch'){s.selected=value;if(rememberScope)switchAccount(i);else{s.sheet='switch';s.remember=false;render(i);}return;}
 if(name==='confirm-switch'){switchAccount(i);return;}
 if(name==='confirm-restart'){s.sheet=null;if(!online){toast(i,'电脑已离线，当前无法重启会话。');return;}toast(i,'示例：已重启并接续 3 个 Claude 会话。');return;}
 if(name==='refresh'){if(!online){toast(i,'无法连接电脑，保留上次用量。');return;}refreshed=true;renderAll();toast(i,'示例用量已刷新');return;}
 if(name==='toggle'&&value==='offline'){online=!online;renderAll();return;}
 if(name==='toggle'&&value==='execution'){const pre=document.querySelector(`#phone-${i} .execution pre`);pre.hidden=!pre.hidden;return;}
 if(name==='reconnect'){toast(i,'正在检查连接…');setTimeout(()=>{online=true;renderAll();toast(i,'演示：连接已恢复，草稿仍保留。');},900);return;}
 if(name==='suggest'){s.draft=value;render(i);document.querySelector(`#phone-${i} textarea`).focus();return;}
 if(name==='permission-demo'){s.sheet=null;s.permission=true;s.decision='';s.tab='dialogue';render(i);return;}
 if(name==='permission'){if(!online){toast(i,'电脑已离线，暂时无法处理。');return;}s.permission=false;s.decision=value==='allow'?'已确认，电脑继续执行。':'本次操作未执行。';toast(i,'示例：'+s.decision);return;}
 if(name==='interrupt'){s.sheet=null;s.replying=false;toast(i,'示例：当前任务已中断，对话已保留。');return;}
 if(name==='key'){if(!online){toast(i,'电脑已离线，按键未发送。');return;}toast(i,`示例：已发送 ${value}`);return;}
 if(name==='create-demo'){if(!online){toast(i,'电脑已离线，无法创建会话。');return;}s.sheet=null;s.session=1;s.view='chat';s.messages=[];render(i);toast(i,'设计示例：新会话入口已打开');return;}
}
function submit(i){const s=states[i];if(!online)return;if(s.replying){s.replying=false;render(i);return;}const text=s.draft.trim();if(!text){toast(i,'先输入你想继续聊的内容');document.querySelector(`#phone-${i} textarea`).focus();return;}s.messages.push({kind:'user',text});s.draft='';s.replying=true;render(i);scrollEnd(i);setTimeout(()=>{if(!s.replying)return;s.replying=false;if(!online){s.messages.push({kind:'assistant',text:'示例：连接中断，暂时无法获取电脑的回复。恢复连接后继续同步。'});}else{s.messages.push({kind:'assistant',text:'收到。我会接着当前会话继续。这里是交互演示，消息没有发送到真实电脑。'});}render(i);scrollEnd(i);},1100);}
function scrollEnd(i){requestAnimationFrame(()=>{const el=document.querySelector(`#phone-${i} .chat-content`);if(el)el.scrollTop=el.scrollHeight;});}
document.querySelectorAll('.phone').forEach((el,i)=>{
 el.addEventListener('click',e=>{const target=e.target.closest('[data-action]');if(target)action(i,target.dataset.action);else if(e.target.classList.contains('sheet-layer'))action(i,'close');});
 el.addEventListener('input',e=>{if(e.target.matches('textarea'))states[i].draft=e.target.value;if(e.target.matches('[data-search]')){states[i].query=e.target.value;el.querySelector('.session-results').innerHTML=sessionRows(states[i]);}});
 el.addEventListener('change',e=>{if(e.target.matches('[data-remember]'))states[i].remember=e.target.checked;});
 el.addEventListener('submit',e=>{e.preventDefault();submit(i);});
 el.addEventListener('keydown',e=>{if(states[i].sheet&&e.key==='Escape'){e.preventDefault();action(i,'close');}if(states[i].sheet&&e.key==='Tab'){const fs=[...el.querySelectorAll('.sheet button,.sheet input')].filter(x=>!x.disabled);const first=fs[0],last=fs.at(-1);if(e.shiftKey&&[first,el.querySelector('.sheet')].includes(document.activeElement)){e.preventDefault();last.focus();}else if(!e.shiftKey&&document.activeElement===last){e.preventDefault();first.focus();}}if(e.target.matches('textarea')&&(e.metaKey||e.ctrlKey)&&e.key==='Enter'){e.preventDefault();submit(i);}});
});
function mode(single){document.body.classList.toggle('single',single);document.querySelector('.single-toolbar').hidden=!single;document.querySelector('#overview').classList.toggle('selected',!single);document.querySelector('#single').classList.toggle('selected',single);}
document.querySelector('#single').onclick=()=>mode(true);document.querySelector('#overview').onclick=()=>mode(false);
document.querySelectorAll('[data-preview]').forEach(b=>b.onclick=()=>preview(b.dataset.preview));
function preview(view){const s=states[0];s.sheet=null;s.permission=false;s.decision='';online=view!=='offline';s.view=view==='offline'?'device':view==='permission'?'chat':view;if(view==='permission')s.permission=true;document.querySelectorAll('[data-preview]').forEach(b=>b.classList.toggle('active',b.dataset.preview===view));renderAll();}
const params=new URLSearchParams(location.search);if(params.has('screen')){mode(true);preview(params.get('screen'));}if(params.has('capture'))document.body.classList.add('capture');
renderAll();
