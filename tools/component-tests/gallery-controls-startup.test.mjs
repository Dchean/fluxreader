import assert from 'node:assert/strict';
import { componentTest, mount, unmount, flush, waitFor, resetStore, useAppStore, actWarnings } from './harness.mjs';
import { installBackend, seedFromRows, mkRow, setPlan, resetPlan, deferred, clearInvokeCalls, invokeCalls, invokeCount } from './fixtures.mjs';

componentTest('U2：下拉键盘活动项与唯一option ID关联，Home/End/Enter/Esc/Tab不混淆焦点与选中', {timeout:20000}, async()=>{
  resetStore();
  const {createElement,Fragment,act}=await import('react');
  const {FluxDropdown}=await import('../../src/components/primitives.tsx');
  const selected=[];
  const options=['A','B','C','D'].map(x=>({value:x,label:x}));
  const entry=await mount(createElement(Fragment,null,
    createElement(FluxDropdown,{options,value:'B',ariaLabel:'Dropdown one',onChange:x=>selected.push(x)}),
    createElement(FluxDropdown,{options,value:'A',ariaLabel:'Dropdown two',onChange:x=>selected.push(x)}),
    createElement('button',{id:'after-dropdown'},'After'),
  ));
  const trigger=entry.container.querySelector('[aria-label="Dropdown one"]');
  const key=async name=>act(async()=>trigger.dispatchEvent(new KeyboardEvent('keydown',{key:name,bubbles:true,cancelable:true})));
  trigger.focus();await key('ArrowDown');
  const menuId=trigger.getAttribute('aria-controls'),menu=document.getElementById(menuId);
  assert.ok(menu?.matches('[role="listbox"]'));
  const opt=()=>document.getElementById(trigger.getAttribute('aria-activedescendant'));
  assert.equal(opt()?.textContent,'B');assert.equal(document.activeElement,trigger);
  await key('ArrowDown');assert.equal(opt()?.textContent,'C');assert.equal(selected.length,0);
  assert.equal(menu.querySelector('[aria-selected="true"]').textContent,'B','活动项移动不提交选择');
  await key('End');assert.equal(opt()?.textContent,'D');
  await key('Home');assert.equal(opt()?.textContent,'A');
  const ids=[...menu.querySelectorAll('[role="option"]')].map(el=>el.id);
  assert.equal(new Set(ids).size,4);assert.ok(ids.every(Boolean));
  await key('Enter');assert.deepEqual(selected,['A']);assert.equal(trigger.getAttribute('aria-controls'),null);assert.equal(trigger.getAttribute('aria-activedescendant'),null);
  await key(' ');const firstId=trigger.getAttribute('aria-controls');await key('Escape');
  assert.equal(document.getElementById(firstId),null);assert.equal(document.activeElement,trigger);
  await key('ArrowDown');await key('Tab');assert.equal(trigger.getAttribute('aria-expanded'),'false');
  const second=entry.container.querySelector('[aria-label="Dropdown two"]');second.focus();
  await act(async()=>second.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true})));
  assert.notEqual(second.getAttribute('aria-controls'),menuId,'多个下拉不共享option关系ID');
  await act(async()=>entry.container.querySelector('#after-dropdown').focus());
  assert.equal(second.getAttribute('aria-expanded'),'false','焦点离开时portal菜单关闭');
});

componentTest('About真实设置入口：取得本地版本后点击检查、下载走真实opener，失败可恢复', {timeout:25000}, async()=>{
  installBackend();resetPlan();resetStore();clearInvokeCalls();seedFromRows([mkRow({id:101})]);
  globalThis.__APP_VERSION_FOR_TESTS__=Promise.resolve('0.18.7');
  globalThis.__OPEN_URLS__=[];
  let fail=false;
  setPlan({check_for_updates:()=>fail?Promise.reject(new Error('fixture unavailable')):Promise.resolve({version:'0.18.8',url:'https://github.com/Dchean/fluxreader/releases/tag/v0.18.8'})});
  const {createElement,act}=await import('react');
  const {SettingsModal}=await import('../../src/components/SettingsModal.tsx');
  try{
    const entry=await mount(createElement(SettingsModal));
    await act(async()=>useAppStore.getState().openSettings());
    const about=[...entry.container.querySelectorAll('.settings-nav-item')].find(b=>b.textContent==='关于');
    assert.ok(about);await act(async()=>about.click());await flush(3);
    assert.match(entry.container.querySelector('.settings-content-pane').textContent,/FluxReader v0.18.7/);
    const check=()=>[...entry.container.querySelectorAll('button')].find(b=>b.textContent==='检查更新');
    fail=true;await act(async()=>check().click());await flush(3);
    assert.match(entry.container.querySelector('.settings-content-pane').textContent,/检查失败/);
    fail=false;await act(async()=>check().click());await flush(3);
    const download=[...entry.container.querySelectorAll('button')].find(b=>b.textContent.includes('0.18.8 可用'));
    assert.ok(download);await act(async()=>download.click());await flush(2);
    assert.deepEqual(globalThis.__OPEN_URLS__,['https://github.com/Dchean/fluxreader/releases/tag/v0.18.8']);
    assert.equal(invokeCount('check_for_updates'),2);
  }finally{delete globalThis.__APP_VERSION_FOR_TESTS__;resetPlan();}
});

componentTest('设置内容卸载：About版本回包迟到、已打开dropdown切页后不留portal或错误', {timeout:20000}, async()=>{
  installBackend();resetPlan();resetStore();
  const pending=deferred();globalThis.__APP_VERSION_FOR_TESTS__=pending.promise;
  const {createElement,act}=await import('react');
  const {SettingsModal}=await import('../../src/components/SettingsModal.tsx');
  try{
    const entry=await mount(createElement(SettingsModal));
    await act(async()=>useAppStore.getState().openSettingsTab('about'));
    await act(async()=>useAppStore.getState().switchSettingsTab('reading'));
    await act(async()=>pending.resolve('0.18.7'));
    const trigger=entry.container.querySelector('[role="combobox"]');assert.ok(trigger);trigger.focus();
    await act(async()=>trigger.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true,cancelable:true})));
    assert.ok(document.querySelector('.flux-dropdown-menu'));
    await act(async()=>useAppStore.getState().switchSettingsTab('shortcuts'));
    assert.equal(document.querySelector('.flux-dropdown-menu'),null);
    assert.match(entry.container.querySelector('.settings-content-pane').textContent,/Ctrl\+K/);
    const before=actWarnings.length;
    await unmount(entry);await flush(2);
    assert.equal(actWarnings.length,before,'受控迟到回包和卸载清理均在act内');
  }finally{delete globalThis.__APP_VERSION_FOR_TESTS__;resetPlan();}
});

for (const navigation of ['switch', 'close']) {
  for (const outcome of ['resolve', 'reject']) {
    componentTest(`About检查在途后${navigation}/${outcome}：迟到结果不污染新页或重开实例`, {timeout:20000}, async()=>{
      installBackend();resetPlan();resetStore();clearInvokeCalls();
      globalThis.__APP_VERSION_FOR_TESTS__=Promise.resolve('0.18.7');
      const pending=deferred();
      setPlan({check_for_updates:()=>pending.promise});
      const {createElement,act}=await import('react');
      const {SettingsModal}=await import('../../src/components/SettingsModal.tsx');
      try{
        const entry=await mount(createElement(SettingsModal));
        await act(async()=>useAppStore.getState().openSettingsTab('about'));await flush(2);
        const check=[...entry.container.querySelectorAll('button')].find(b=>b.textContent==='检查更新');
        await act(async()=>check.click());await flush(2);
        assert.equal(invokeCount('check_for_updates'),1);
        assert.match(entry.container.querySelector('.settings-content-pane').textContent,/正在检查/);
        await act(async()=>navigation==='switch'
          ? useAppStore.getState().switchSettingsTab('shortcuts') : useAppStore.getState().closeSettings());
        const beforeWarnings=actWarnings.length;
        await act(async()=>outcome==='resolve'
          ? pending.resolve({version:'0.18.8',url:'https://github.com/Dchean/fluxreader/releases/tag/v0.18.8'})
          : pending.reject(new Error('fixture check failed after navigation')));
        await flush(3);
        assert.equal(actWarnings.length,beforeWarnings,'受控回包及清理不新增act警告');
        if(navigation==='switch'){
          assert.match(entry.container.querySelector('.settings-content-pane').textContent,/Ctrl\+K/);
          assert.doesNotMatch(entry.container.querySelector('.settings-content-pane').textContent,/0\.18\.8|检查失败/);
          await act(async()=>useAppStore.getState().switchSettingsTab('about'));await flush(2);
          assert.ok([...entry.container.querySelectorAll('button')].some(b=>b.textContent==='检查更新'),
            '切回的新About实例保持空闲，不接受已卸载实例状态');
        }else{
          assert.equal(useAppStore.getState().settingsOpen,false,'回包不重开设置');
          await act(async()=>useAppStore.getState().openSettings());
          assert.match(entry.container.querySelector('.settings-content-pane').textContent,
            outcome==='resolve' ? /0\.18\.8 可用/ : /检查失败/,
            '关闭设置保留常驻面板，重开能看到本次用户检查的终态');
        }
        assert.equal(invokeCount('check_for_updates'),1,'导航不自动重复出网');
        if(outcome==='reject')assert.ok(useAppStore.getState().toasts.some(t=>t.text.includes('fixture check failed after navigation')),
          '用户已发检查的失败仍报告，不伪装成功');
        await unmount(entry);
      }finally{delete globalThis.__APP_VERSION_FOR_TESTS__;resetPlan();}
    });
  }
}

componentTest('NF4条件时序防御：设置未落定时后台接手也等门，接受未读启动设置后自动首拉', {timeout:25000}, async()=>{
  installBackend();resetPlan();resetStore();clearInvokeCalls();
  const {resetAppSettingsWriteStateForTests}=await import('../../dist-test/store/slices/settings.js');resetAppSettingsWriteStateForTests();
  seedFromRows([],{layout:'article'});
  const rows=Array.from({length:1001},(_,i)=>mkRow({id:10001+i,feed_id:10,title:`startup-${i}`,is_read:i>=501,published_at:new Date(Date.UTC(2026,9,1)+i*1000).toISOString()}));
  const settings=deferred(),queries=[];
  setPlan({get_setting:({key})=>key==='app_settings'?settings.promise:Promise.resolve(null),list_articles:({args})=>{
    queries.push({...args});let selected=[...rows].sort((a,b)=>b.id-a.id);if(args.only_unread)selected=selected.filter(row=>!row.is_read);if(args.last_id!=null)selected=selected.filter(row=>row.id<args.last_id);return Promise.resolve(selected.slice(args.offset??0,(args.offset??0)+(args.limit??500)));
  }});
  useAppStore.setState(s=>({activeViewFilter:'all',timelineFilter:'all',settings:{...s.settings,smartDedup:false},dataLoading:true,entries:[],articlesCursor:{}}));
  const {createElement,act}=await import('react');const {default:App}=await import('../../src/App.tsx');
  try{
    const entry=await mount(createElement(App));await flush(3);
    let listeners;
    await act(async()=>{listeners=globalThis.__DISPATCH_TAURI_EVENT__('feeds-updated',{new_articles:0,failed_feeds:0,merged_states:1});});await flush(3);
    assert.equal(listeners,1);assert.ok(queries.length>=2,'启动与事件接手查询均已发出');
    assert.equal(useAppStore.getState().entries.length,0,'设置在途时不能按默认all落地');
    await act(async()=>settings.resolve(JSON.stringify({startupView:'unread',hideReadOnStartup:true,smartDedup:false})));
    await waitFor(()=>entry.container.querySelectorAll('.article-card').length>0,{label:'未读启动视图自动首拉',timeout:8000});
    assert.equal(useAppStore.getState().activeViewFilter,'unread');
    assert.ok(queries.some(q=>q.only_unread===true),'真实发出未读谓词首拉');
    assert.equal(useAppStore.getState().dataLoading,false);
    assert.ok(useAppStore.getState().entries.some(e=>!e.isRead));
    assert.equal(invokeCalls.some(c=>c.cmd==='ai_translate'||c.cmd==='ai_summarize'),false);
  }finally{resetPlan();}
});
