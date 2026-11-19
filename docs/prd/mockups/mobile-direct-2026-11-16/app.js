(function () {
  'use strict';

  var STORAGE_KEY = 'matou_mobile_direct_proto_v1';
  var DEVICE_NAME = '家里的Mac';

  var STATUS_META = {
    running: { label: '运行中', cls: 'is-running' },
    confirm: { label: '待确认', cls: 'is-confirm' },
    input: { label: '待输入', cls: 'is-input' },
    done: { label: '已完成', cls: 'is-done' },
    stopped: { label: '已停止', cls: 'is-stopped' }
  };

  function buildInitialData() {
    return {
      sessions: [
        {
          id: 'alt',
          code: 'alt',
          title: '替代方案',
          status: 'running',
          statusDetail: '',
          parentId: 'main',
          branchNote: '继承上下文·独立工作副本',
          highlight: true,
          workspace: 'Matou',
          item: '登录改版',
          canvas: '登录流程',
          updatedAt: '刚刚',
          summary: '验证码优先、保留密码兜底；3个文件已改动，测试 12/13 通过',
          diffExpanded: false,
          diffFiles: [
            {
              path: 'src/pages/login/LoginEntry.tsx',
              stat: '+64 -18',
              snippet: '默认聚焦手机号与验证码输入，底部保留「使用密码登录」次要切换入口'
            },
            {
              path: 'src/pages/login/CodeVerifyPanel.tsx',
              stat: '+52 -12',
              snippet: '新增 60s 重发倒计时与输入防抖，兼容弱网重试按钮状态'
            },
            {
              path: 'src/stores/authFlowStore.ts',
              stat: '+26 -8',
              snippet: '独立副本持久化用户上次登录偏好，未显式选密码时优先验证码'
            }
          ],
          messages: [
            {
              id: 'alt-m1',
              role: 'user',
              sender: '我',
              text: '要求：登录改版改为「验证码优先，密码保留」。新用户和常用设备默认走手机号 + 验证码一步登录，同时在底部保留清晰的密码登录切换，确保老用户习惯不被打断。'
            },
            {
              id: 'alt-m2',
              role: 'ai',
              sender: 'Matou 助手',
              text: '已从「主方案」分出独立工作副本并继承上下文。调整结构如下：\n1. 首屏仅保留手机号与 6 位验证码主路径，降低首屏表单密度。\n2. 底部保留「使用密码登录」辅助按钮，点击就地切换而不跳转页面。\n3. 本地记忆最近一次成功登录的方式，减少重复切换成本。'
            },
            {
              id: 'alt-card-diff',
              role: 'card-diff'
            },
            {
              id: 'alt-m3',
              role: 'ai',
              sender: 'Matou 助手',
              text: '已同步更新登录交互说明，并自动触发登录流程回归测试与弱网子分支压测。'
            },
            {
              id: 'alt-attach',
              role: 'attachment',
              name: '登录改版首屏交互稿.pdf（本地演示附件）'
            },
            {
              id: 'alt-card-test',
              role: 'card-test',
              title: '登录流程自动化测试 · 12 项通过 1 失败',
              desc: '失败项：TC-09 弱网超时后验证码重发按钮未复位',
              targetId: 'test',
              btnLabel: '打开测试会话'
            },
            {
              id: 'alt-card-weak',
              role: 'card-weak',
              title: '弱网验证子分支运行中',
              desc: '正在模拟 3G 丢包场景下的倒计时状态恢复',
              targetId: 'weak',
              btnLabel: '进入弱网验证'
            },
            {
              id: 'alt-m4',
              role: 'user',
              sender: '我',
              text: '先看下第 9 条用例在弱网下为什么没重置倒计时，如果只是 finally 漏了清理，等会直接修完再合回主方案。'
            },
            {
              id: 'alt-m5',
              role: 'ai',
              sender: 'Matou 助手',
              text: '正在跟踪 CodeVerifyPanel.tsx 中的 timerRef 清理路径：请求抛出 AbortError 时提前 return 跳过了状态复位。当前正在等待弱网分支的复现日志以确认修复范围…'
            }
          ]
        },
        {
          id: 'main',
          code: 'main',
          title: '主方案',
          status: 'confirm',
          statusDetail: '',
          parentId: null,
          branchNote: '',
          highlight: false,
          workspace: 'Matou',
          item: '登录改版',
          canvas: '登录流程',
          updatedAt: '13:18',
          summary: '密码与验证码双 Tab 并列方案已就绪，待确认是否采用替代方案',
          messages: [
            {
              id: 'main-m1',
              role: 'user',
              sender: '我',
              text: '先梳理新版登录页的基础信息架构：需要同时支持手机号验证码和账号密码，保证埋点口径一致。'
            },
            {
              id: 'main-m2',
              role: 'ai',
              sender: 'Matou 助手',
              text: '主方案采用顶部双 Tab（密码登录 / 验证码登录）并列结构，已完成基础校验与埋点字段对齐。后续你在「替代方案 (alt)」中进一步验证「验证码优先」的精简首屏版本。'
            },
            {
              id: 'main-m3',
              role: 'ai',
              sender: 'Matou 助手',
              text: '当前主方案保持冻结，等待你确认保留双 Tab，或由「替代方案」验证通过后合并覆盖。'
            },
            {
              id: 'main-card-branch',
              role: 'card-jump',
              title: '下游分支：替代方案 (alt)',
              desc: '继承本会话上下文的独立工作副本，正在运行中',
              targetId: 'alt',
              btnLabel: '查看替代方案'
            }
          ]
        },
        {
          id: 'weak',
          code: 'weak',
          title: '弱网验证',
          status: 'running',
          statusDetail: '',
          parentId: 'alt',
          branchNote: '继承上下文·独立工作副本',
          highlight: false,
          workspace: 'Matou',
          item: '登录改版',
          canvas: '登录流程',
          updatedAt: '2分钟前',
          summary: '正在模拟 3G 丢包与验证码重发退避计时器（第 4/6 项）',
          messages: [
            {
              id: 'weak-m1',
              role: 'user',
              sender: '我',
              text: '从替代方案分出来专门跑一遍 3G 弱网和 30% 丢包，看看验证码重发和切回密码会不会卡死。'
            },
            {
              id: 'weak-m2',
              role: 'ai',
              sender: 'Matou 助手',
              text: '已继承「替代方案 (alt)」上下文启动独立弱网沙箱：\n• 用例 1-3：首屏渲染与切换密码登录（通过）\n• 用例 4：请求 8s 超时后重试按钮锁定检查（复现异常，正在采集状态快照）'
            },
            {
              id: 'weak-m3',
              role: 'ai',
              sender: 'Matou 助手',
              text: '正在执行第 5/6 项弱网断流恢复验证… 如需人工介入修改，可点击下方停止或返回替代方案。'
            }
          ]
        },
        {
          id: 'test',
          code: 'test',
          title: '测试',
          status: 'done',
          statusDetail: '12项通过 1失败',
          parentId: null,
          branchNote: '',
          highlight: false,
          workspace: 'Matou',
          item: '登录改版',
          canvas: '登录流程',
          updatedAt: '6分钟前',
          summary: '已完成 12 项通过 1 失败：弱网超时重发按钮状态未重置',
          messages: [
            {
              id: 'test-m1',
              role: 'system',
              text: '本会话为同画布「登录流程」下的独立测试会话（无上游继承分支）'
            },
            {
              id: 'test-m2',
              role: 'ai',
              sender: '测试执行器',
              text: '登录流程回归测试套件已运行完毕，共 13 项：\n• 通过 12 项：首屏聚焦、验证码校验、密码切回、偏好记忆、异常错误提示等均正常。\n• 失败 1 项：TC-09 弱网超时后「重新发送」按钮仍停留在 loading 态。'
            },
            {
              id: 'test-m3',
              role: 'ai',
              sender: '测试执行器',
              text: '失败堆栈摘要：\nCodeVerifyPanel.handleResend -> catch(TimeoutError) 未触发 setSending(false)，建议在 finally 中统一重置发送锁。'
            },
            {
              id: 'test-card-back',
              role: 'card-jump',
              title: '去对应实现会话处理修复',
              desc: '或直接点击左上角返回按钮，按访问栈回到上一会话',
              targetId: 'alt',
              btnLabel: '前往替代方案 (alt)'
            }
          ]
        },
        {
          id: 'recovery',
          code: 'recovery',
          title: '账户恢复',
          status: 'confirm',
          statusDetail: '',
          parentId: null,
          branchNote: '',
          highlight: false,
          workspace: 'Matou',
          item: '登录改版',
          canvas: '账户恢复',
          updatedAt: '28分钟前',
          summary: '手机号停用时支持绑定邮箱+人工申诉双通道，待确认文案',
          messages: [
            {
              id: 'rec-m1',
              role: 'user',
              sender: '我',
              text: '登录改版后如果用户原手机号已停用，账户恢复画布需要补充绑定邮箱验证与工单申诉入口。'
            },
            {
              id: 'rec-m2',
              role: 'ai',
              sender: 'Matou 助手',
              text: '已在「登录改版 / 账户恢复」画布整理两段式引导：\n1. 优先校验历史绑定邮箱，通过后可直接换绑新手机号。\n2. 未绑定邮箱时进入轻量身份核验表单，预计 24 小时内反馈。\n当前文案与按钮层级已就绪，等待确认。'
            }
          ]
        },
        {
          id: 'api',
          code: 'api',
          title: '接口联调',
          status: 'input',
          statusDetail: '',
          parentId: null,
          branchNote: '',
          highlight: false,
          workspace: '服务端',
          item: '消息同步',
          canvas: '接口验证',
          updatedAt: '1小时前',
          summary: '等待补充增量游标 ack_seq 异常回退参数示例',
          messages: [
            {
              id: 'api-m1',
              role: 'user',
              sender: '我',
              text: '核对一下消息同步接口 /v2/sync/delta 在断线重连时的游标回退字段。'
            },
            {
              id: 'api-m2',
              role: 'ai',
              sender: 'Matou 助手',
              text: '目前服务端返回了 last_seq 与 reset_flag，但缺少客户端上报 ack_seq 非法值时的错误码约定。请补充一段预期的异常响应 JSON 示例，我来生成联调校验脚本。'
            }
          ]
        },
        {
          id: 'release',
          code: 'release',
          title: '发布检查',
          status: 'confirm',
          statusDetail: '',
          parentId: null,
          branchNote: '',
          highlight: false,
          workspace: 'Matou',
          item: '上线发布',
          canvas: '发布流程',
          updatedAt: '昨天',
          summary: '灰度包校验与回滚脚本已核对，待确认 18:00 发布窗口',
          messages: [
            {
              id: 'rel-m1',
              role: 'ai',
              sender: 'Matou 助手',
              text: 'Matou 上线发布检查清单已完成 5/5 项：\n1. 构建产物签名校验通过\n2. 灰度开关默认 5% 流量\n3. 回滚脚本在预发环境演练通过\n等待你确认今晚 18:00 灰度发布窗口。'
            }
          ]
        }
      ],
      drafts: {},
      scrollPositions: {}
    };
  }

  function loadState() {
    var base = buildInitialData();
    try {
      var raw = window.sessionStorage.getItem(STORAGE_KEY);
      if (raw) {
        var parsed = JSON.parse(raw);
        if (parsed && Array.isArray(parsed.sessions) && parsed.sessions.length > 0) {
          base.sessions = parsed.sessions;
        }
        if (parsed && parsed.drafts && typeof parsed.drafts === 'object') {
          base.drafts = parsed.drafts;
        }
        if (parsed && parsed.scrollPositions && typeof parsed.scrollPositions === 'object') {
          base.scrollPositions = parsed.scrollPositions;
        }
      }
    } catch (e) {
      // Ignore storage errors in restricted environments
    }
    return {
      sessions: base.sessions,
      drafts: base.drafts,
      scrollPositions: base.scrollPositions,
      activeTab: 'sessions',
      activeFilter: 'recent',
      searchOpen: false,
      searchQuery: '',
      currentSessionId: null,
      visitStack: [{ view: 'tab', tab: 'sessions' }],
      sheetMode: null,
      sheetAccordionExpanded: {}
    };
  }

  var state = loadState();
  var toastTimer = null;
  var lastFocusedBeforeSheet = null;

  function persistState() {
    try {
      window.sessionStorage.setItem(
        STORAGE_KEY,
        JSON.stringify({
          sessions: state.sessions,
          drafts: state.drafts,
          scrollPositions: state.scrollPositions
        })
      );
    } catch (e) {
      // Ignore storage quota errors
    }
  }

  // DOM References
  var elViewTabs = document.getElementById('view-tabs');
  var elViewDetail = document.getElementById('view-detail');

  var elTabPanels = {
    sessions: document.getElementById('tab-sessions'),
    workspaces: document.getElementById('tab-workspaces'),
    me: document.getElementById('tab-me')
  };
  var elNavBtns = document.querySelectorAll('.nav-item');

  var elBtnToggleSearch = document.getElementById('btn-toggle-search');
  var elSearchWrap = document.getElementById('home-search-wrap');
  var elSearchInput = document.getElementById('home-search-input');
  var elBtnClearSearch = document.getElementById('btn-clear-search');
  var elBtnNewSession = document.getElementById('btn-new-session');
  var elHomeFilters = document.getElementById('home-filters');
  var elSessionList = document.getElementById('session-list');
  var elHomeEmpty = document.getElementById('home-empty');
  var elBtnResetFilters = document.getElementById('btn-reset-filters');

  var elWorkspacesTree = document.getElementById('workspaces-tree');
  var elWorkspaceSummaryCount = document.getElementById('workspace-summary-count');
  var elBtnResetDemo = document.getElementById('btn-reset-demo');

  var elBtnDetailBack = document.getElementById('btn-detail-back');
  var elDetailTitle = document.getElementById('detail-title');
  var elDetailStatusBadge = document.getElementById('detail-status-badge');
  var elBtnOpenLocation = document.getElementById('btn-open-location');
  var elDetailPathText = document.getElementById('detail-path-text');
  var elBtnDeviceStatus = document.getElementById('btn-device-status');
  var elRelationPrimary = document.getElementById('relation-primary');
  var elRelationSub = document.getElementById('relation-sub');
  var elBtnOpenSwitcher = document.getElementById('btn-open-switcher');
  var elDetailScroll = document.getElementById('detail-scroll');
  var elDetailMessages = document.getElementById('detail-messages');

  var elRunningBanner = document.getElementById('running-banner');
  var elRunningBannerText = document.getElementById('running-banner-text');
  var elBtnStopSession = document.getElementById('btn-stop-session');
  var elBtnComposerAttach = document.getElementById('btn-composer-attach');
  var elComposerInput = document.getElementById('composer-input');
  var elBtnSendMessage = document.getElementById('btn-send-message');

  var elSheetBackdrop = document.getElementById('sheet-backdrop');
  var elBottomSheet = document.getElementById('bottom-sheet');
  var elBtnSheetBack = document.getElementById('btn-sheet-back');
  var elSheetTitle = document.getElementById('sheet-title');
  var elSheetSubtitle = document.getElementById('sheet-subtitle');
  var elBtnCloseSheet = document.getElementById('btn-close-sheet');
  var elSheetBody = document.getElementById('sheet-body');
  var elToast = document.getElementById('toast');

  // Helpers
  function escapeHtml(str) {
    return String(str)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function getSessionById(id) {
    for (var i = 0; i < state.sessions.length; i++) {
      if (state.sessions[i].id === id) {
        return state.sessions[i];
      }
    }
    return null;
  }

  function getStatusLabel(session) {
    var meta = STATUS_META[session.status] || STATUS_META.confirm;
    return meta.label;
  }

  function getStatusClass(session) {
    var meta = STATUS_META[session.status] || STATUS_META.confirm;
    return meta.cls;
  }

  function formatPath(session) {
    return session.workspace + ' / ' + session.item + ' / ' + session.canvas;
  }

  function showToast(msg) {
    elToast.textContent = msg;
    elToast.hidden = false;
    if (toastTimer) {
      clearTimeout(toastTimer);
    }
    toastTimer = setTimeout(function () {
      elToast.hidden = true;
    }, 2400);
  }

  function saveCurrentSessionSnapshot() {
    if (!state.currentSessionId) return;
    if (!elViewDetail.hidden) {
      state.scrollPositions[state.currentSessionId] = elDetailScroll.scrollTop;
      state.drafts[state.currentSessionId] = elComposerInput.value;
      persistState();
    }
  }

  function autoResizeComposer() {
    elComposerInput.style.height = 'auto';
    var nextHeight = Math.min(elComposerInput.scrollHeight, 112);
    elComposerInput.style.height = Math.max(44, nextHeight) + 'px';
  }

  function updateSendButtonState() {
    elBtnSendMessage.disabled = elComposerInput.value.trim().length === 0;
  }

  // Derived filter counts
  function getFilterCounts() {
    var recent = state.sessions.length;
    var pending = 0;
    var running = 0;
    for (var i = 0; i < state.sessions.length; i++) {
      var st = state.sessions[i].status;
      if (st === 'confirm' || st === 'input') {
        pending++;
      } else if (st === 'running') {
        running++;
      }
    }
    return {
      recent: recent,
      pending: pending,
      running: running
    };
  }

  function getFilteredSessions() {
    var q = state.searchQuery.trim().toLowerCase();
    return state.sessions.filter(function (s) {
      if (state.activeFilter === 'pending' && s.status !== 'confirm' && s.status !== 'input') {
        return false;
      }
      if (state.activeFilter === 'running' && s.status !== 'running') {
        return false;
      }
      if (!q) return true;
      var haystack = [s.title, s.code, s.workspace, s.item, s.canvas, s.summary, getStatusLabel(s)]
        .join(' ')
        .toLowerCase();
      return haystack.indexOf(q) !== -1;
    });
  }

  // Render Home
  function renderFilters() {
    var counts = getFilterCounts();
    var defs = [
      { key: 'recent', label: '最近', count: counts.recent },
      { key: 'pending', label: '待处理', count: counts.pending },
      { key: 'running', label: '运行中', count: counts.running }
    ];
    elHomeFilters.innerHTML = defs
      .map(function (d) {
        var active = state.activeFilter === d.key;
        return (
          '<button class="filter-chip' +
          (active ? ' is-active' : '') +
          '" type="button" role="tab" aria-selected="' +
          (active ? 'true' : 'false') +
          '" data-filter="' +
          d.key +
          '">' +
          '<span>' +
          escapeHtml(d.label) +
          '</span>' +
          '<span class="filter-count">' +
          d.count +
          '</span>' +
          '</button>'
        );
      })
      .join('');
  }

  function renderSessionList() {
    var list = getFilteredSessions();
    if (list.length === 0) {
      elSessionList.innerHTML = '';
      elHomeEmpty.hidden = false;
      return;
    }
    elHomeEmpty.hidden = true;
    elSessionList.innerHTML = list
      .map(function (s) {
        var parent = s.parentId ? getSessionById(s.parentId) : null;
        var draftText = state.drafts[s.id] ? state.drafts[s.id].trim() : '';
        var footTags = [];
        if (parent) {
          footTags.push('<span class="mini-pill is-branch">分支自 ' + escapeHtml(parent.title) + '</span>');
        }
        if (draftText) {
          footTags.push('<span class="mini-pill is-draft">有草稿</span>');
        }

        return (
          '<button class="session-card' +
          (s.highlight ? ' is-highlight' : '') +
          '" type="button" role="listitem" data-session-id="' +
          escapeHtml(s.id) +
          '">' +
          '<div class="session-card-head">' +
          '<div class="session-name-wrap">' +
          '<span class="session-title">' +
          escapeHtml(s.title) +
          '</span>' +
          '</div>' +
          '<span class="status-badge ' +
          getStatusClass(s) +
          '"><span class="status-badge-dot" aria-hidden="true"></span>' +
          escapeHtml(getStatusLabel(s)) +
          '</span>' +
          '</div>' +
          '<div class="session-meta-row">' +
          '<span class="session-path">' +
          escapeHtml(formatPath(s)) +
          '</span>' +
          '<span class="session-time">' +
          escapeHtml(s.updatedAt) +
          '</span>' +
          '</div>' +
          '<p class="session-summary">' +
          escapeHtml(s.summary) +
          '</p>' +
          (footTags.length ? '<div class="session-foot-tags">' + footTags.join('') + '</div>' : '') +
          '</button>'
        );
      })
      .join('');
  }

  // Group sessions into Workspace -> Item -> Canvas
  function groupSessionsHierarchy(sessions) {
    var workspaces = [];
    var wsMap = {};
    for (var i = 0; i < sessions.length; i++) {
      var s = sessions[i];
      if (!wsMap[s.workspace]) {
        wsMap[s.workspace] = { name: s.workspace, items: [], itemMap: {}, count: 0 };
        workspaces.push(wsMap[s.workspace]);
      }
      var ws = wsMap[s.workspace];
      ws.count++;
      if (!ws.itemMap[s.item]) {
        ws.itemMap[s.item] = { name: s.item, canvases: [], canvasMap: {} };
        ws.items.push(ws.itemMap[s.item]);
      }
      var it = ws.itemMap[s.item];
      if (!it.canvasMap[s.canvas]) {
        it.canvasMap[s.canvas] = { name: s.canvas, sessions: [] };
        it.canvases.push(it.canvasMap[s.canvas]);
      }
      it.canvasMap[s.canvas].sessions.push(s);
    }
    return workspaces;
  }

  function renderWorkspaces() {
    var hierarchy = groupSessionsHierarchy(state.sessions);
    elWorkspaceSummaryCount.textContent = hierarchy.length + ' 个工作空间 · ' + state.sessions.length + ' 个会话';
    elWorkspacesTree.innerHTML = hierarchy
      .map(function (ws) {
        var itemsHtml = ws.items
          .map(function (it) {
            var canvasesHtml = it.canvases
              .map(function (cv) {
                var rowsHtml = cv.sessions
                  .map(function (s) {
                    return (
                      '<button class="ws-session-row" type="button" data-session-id="' +
                      escapeHtml(s.id) +
                      '">' +
                      '<div class="ws-session-left">' +
                      '<span class="ws-session-name">' +
                      escapeHtml(s.title) +
                      '</span>' +
                      '</div>' +
                      '<span class="status-badge ' +
                      getStatusClass(s) +
                      '">' +
                      escapeHtml(getStatusLabel(s)) +
                      '</span>' +
                      '</button>'
                    );
                  })
                  .join('');

                return (
                  '<div class="ws-canvas-block">' +
                  '<div class="ws-canvas-label">画布：' +
                  escapeHtml(cv.name) +
                  '</div>' +
                  '<div class="ws-session-list">' +
                  rowsHtml +
                  '</div>' +
                  '</div>'
                );
              })
              .join('');

            return (
              '<div class="ws-item-block">' +
              '<div class="ws-item-title">事项：' +
              escapeHtml(it.name) +
              '</div>' +
              canvasesHtml +
              '</div>'
            );
          })
          .join('');

        return (
          '<section class="ws-group">' +
          '<div class="ws-group-header">' +
          '<div class="ws-group-title">' +
          '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path></svg>' +
          '<span>' +
          escapeHtml(ws.name) +
          '</span>' +
          '</div>' +
          '<span class="ws-group-count">' +
          ws.count +
          ' 个会话</span>' +
          '</div>' +
          itemsHtml +
          '</section>'
        );
      })
      .join('');
  }

  // Relationships calculation (dynamic per current session)
  function getChildren(sessionId) {
    return state.sessions.filter(function (s) {
      return s.parentId === sessionId;
    });
  }

  function getBranchTreeNodes(sessionId) {
    var current = getSessionById(sessionId);
    if (!current) return [];

    // Walk up to root
    var root = current;
    var guard = 0;
    while (root.parentId && guard < 20) {
      var p = getSessionById(root.parentId);
      if (!p) break;
      root = p;
      guard++;
    }

    var result = [];
    var visited = {};
    function dfs(node, depth) {
      if (!node || visited[node.id]) return;
      visited[node.id] = true;
      result.push({ session: node, depth: depth });
      var children = getChildren(node.id);
      for (var i = 0; i < children.length; i++) {
        dfs(children[i], depth + 1);
      }
    }
    dfs(root, 0);
    return result;
  }

  function getRelationBarCopy(session) {
    var parent = session.parentId ? getSessionById(session.parentId) : null;
    var children = getChildren(session.id);

    if (parent) {
      var subText = session.branchNote || '继承上下文 · 独立工作副本';
      if (children.length > 0) {
        subText += ' · 含下游 ' + children.length + ' 个子分支';
      }
      return {
        primary: parent.title + ' → ' + session.title,
        sub: subText
      };
    }

    if (children.length > 0) {
      return {
        primary: session.title + ' · 分支根会话',
        sub: '下游分支：' + children.map(function (c) { return c.title; }).join('、')
      };
    }

    var sameCanvasOthers = state.sessions.filter(function (s) {
      return (
        s.id !== session.id &&
        s.workspace === session.workspace &&
        s.item === session.item &&
        s.canvas === session.canvas
      );
    });
    if (sameCanvasOthers.length > 0) {
      return {
        primary: session.title + ' · 独立会话',
        sub: '同画布「' + session.canvas + '」另有 ' + sameCanvasOthers.length + ' 个会话'
      };
    }

    return {
      primary: session.title + ' · 独立会话',
      sub: formatPath(session)
    };
  }

  // Render Detail View
  function renderDetailMessages(session) {
    elDetailMessages.innerHTML = session.messages
      .map(function (m) {
        if (m.role === 'system') {
          return '<div class="msg-item is-system"><div class="msg-system-pill">' + escapeHtml(m.text) + '</div></div>';
        }
        if (m.role === 'card-diff') {
          var files = session.diffFiles || [];
          var expanded = !!session.diffExpanded;
          var filesHtml = files
            .map(function (f) {
              return (
                '<div class="diff-file-item">' +
                '<div class="diff-file-row">' +
                '<span class="diff-file-path">' +
                escapeHtml(f.path) +
                '</span>' +
                '<span class="diff-stat">' +
                escapeHtml(f.stat) +
                '</span>' +
                '</div>' +
                '<p class="diff-snippet">' +
                escapeHtml(f.snippet) +
                '</p>' +
                '</div>'
              );
            })
            .join('');

          return (
            '<div class="action-card">' +
            '<button class="card-toggle-btn" type="button" data-action="toggle-diff" aria-expanded="' +
            (expanded ? 'true' : 'false') +
            '">' +
            '<div class="card-head-left">' +
            '<div class="card-icon-badge" aria-hidden="true">' +
            '<svg class="icon" viewBox="0 0 24 24"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"></path><path d="M14 2v6h6"></path></svg>' +
            '</div>' +
            '<div class="card-title-block">' +
            '<div class="card-title">' +
            (expanded ? '收起 ' : '展开 ') +
            files.length +
            ' 个文件改动</div>' +
            '<div class="card-sub">LoginEntry / CodeVerifyPanel / authFlowStore · +142 -38</div>' +
            '</div>' +
            '</div>' +
            '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="' +
            (expanded ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6') +
            '"></path></svg>' +
            '</button>' +
            (expanded ? '<div class="diff-list">' + filesHtml + '</div>' : '') +
            '</div>'
          );
        }

        if (m.role === 'card-test' || m.role === 'card-weak' || m.role === 'card-jump') {
          var cardCls =
            m.role === 'card-test' ? ' is-test-card' : m.role === 'card-weak' ? ' is-weak-card' : '';
          var btnCls = m.role === 'card-weak' ? 'card-btn-accent' : 'card-btn-primary';
          var iconPath =
            m.role === 'card-test'
              ? '<path d="M9 11l3 3L22 4"></path><path d="M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11"></path>'
              : '<path d="M13 2L3 14h9l-1 8 10-12h-9l1-8z"></path>';
          return (
            '<div class="action-card' +
            cardCls +
            '">' +
            '<div class="card-head-left">' +
            '<div class="card-icon-badge" aria-hidden="true">' +
            '<svg class="icon" viewBox="0 0 24 24">' +
            iconPath +
            '</svg>' +
            '</div>' +
            '<div class="card-title-block">' +
            '<div class="card-title">' +
            escapeHtml(m.title) +
            '</div>' +
            '<div class="card-sub">' +
            escapeHtml(m.desc) +
            '</div>' +
            '</div>' +
            '</div>' +
            '<div class="card-footer-action">' +
            '<button class="' +
            btnCls +
            '" type="button" data-jump-session="' +
            escapeHtml(m.targetId) +
            '">' +
            escapeHtml(m.btnLabel) +
            '</button>' +
            '</div>' +
            '</div>'
          );
        }

        if (m.role === 'attachment') {
          return (
            '<button class="attachment-chip" type="button" data-action="attachment-demo">' +
            '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M21.44 11.05l-8.49 8.49a5.5 5.5 0 0 1-7.78-7.78l8.49-8.49a3.5 3.5 0 0 1 4.95 4.95l-8.49 8.49a1.5 1.5 0 0 1-2.12-2.12l7.78-7.78"></path></svg>' +
            '<span>' +
            escapeHtml(m.name) +
            '</span>' +
            '</button>'
          );
        }

        var isUser = m.role === 'user';
        return (
          '<div class="msg-item ' +
          (isUser ? 'is-user' : 'is-ai') +
          '">' +
          '<div class="msg-sender">' +
          '<span>' +
          escapeHtml(m.sender || (isUser ? '我' : 'Matou 助手')) +
          '</span>' +
          (m.isDemoReply ? '<span class="demo-tag">演示回复</span>' : '') +
          '</div>' +
          '<div class="msg-bubble">' +
          escapeHtml(m.text) +
          '</div>' +
          '</div>'
        );
      })
      .join('');
  }

  function renderDetailView(sessionId) {
    var session = getSessionById(sessionId);
    if (!session) return;

    elDetailTitle.textContent = session.title;
    elComposerInput.placeholder = '给「' + session.title + '」发消息…';
    elDetailStatusBadge.className = 'status-badge ' + getStatusClass(session);
    elDetailStatusBadge.textContent = getStatusLabel(session);
    elDetailPathText.textContent = formatPath(session);

    var relCopy = getRelationBarCopy(session);
    elRelationPrimary.textContent = relCopy.primary;
    elRelationSub.textContent = relCopy.sub;

    if (session.status === 'running') {
      elRunningBanner.hidden = false;
      elRunningBannerText.textContent = session.title + ' 正在运行中…';
    } else {
      elRunningBanner.hidden = true;
    }

    renderDetailMessages(session);

    // Restore per-session draft
    elComposerInput.value = state.drafts[session.id] || '';
    autoResizeComposer();
    updateSendButtonState();

    // Restore scrollTop (never unconditionally jump to bottom if user already visited/scrolled)
    var hasSavedScroll = Object.prototype.hasOwnProperty.call(state.scrollPositions, session.id);
    window.requestAnimationFrame(function () {
      if (hasSavedScroll) {
        elDetailScroll.scrollTop = state.scrollPositions[session.id];
      } else {
        elDetailScroll.scrollTop = elDetailScroll.scrollHeight;
      }
    });
  }

  // Navigation
  function switchTab(tabName) {
    saveCurrentSessionSnapshot();
    state.activeTab = tabName;
    state.currentSessionId = null;
    state.visitStack = [{ view: 'tab', tab: tabName }];

    elViewDetail.hidden = true;
    elViewTabs.hidden = false;

    Object.keys(elTabPanels).forEach(function (k) {
      elTabPanels[k].hidden = k !== tabName;
    });
    elNavBtns.forEach(function (btn) {
      var active = btn.getAttribute('data-tab') === tabName;
      btn.classList.toggle('is-active', active);
      btn.setAttribute('aria-selected', active ? 'true' : 'false');
    });

    if (tabName === 'sessions') {
      renderFilters();
      renderSessionList();
    } else if (tabName === 'workspaces') {
      renderWorkspaces();
    }
  }

  function openSession(sessionId, options) {
    var target = getSessionById(sessionId);
    if (!target) return;

    options = options || {};
    saveCurrentSessionSnapshot();

    if (!options.fromPop) {
      var top = state.visitStack[state.visitStack.length - 1];
      if (!top || top.view !== 'detail' || top.sessionId !== sessionId) {
        state.visitStack.push({ view: 'detail', sessionId: sessionId });
      }
    }

    state.currentSessionId = sessionId;
    elViewTabs.hidden = true;
    elViewDetail.hidden = false;
    renderDetailView(sessionId);
  }

  function goBack() {
    saveCurrentSessionSnapshot();
    if (state.visitStack.length > 1) {
      state.visitStack.pop();
    }
    var prev = state.visitStack[state.visitStack.length - 1];
    if (prev && prev.view === 'detail' && getSessionById(prev.sessionId)) {
      openSession(prev.sessionId, { fromPop: true });
    } else {
      var tab = prev && prev.tab ? prev.tab : state.activeTab || 'sessions';
      switchTab(tab);
    }
  }

  // Bottom Sheet + Focus Trap + Escape
  function getFocusableInSheet() {
    var selectors =
      'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
    var nodes = elBottomSheet.querySelectorAll(selectors);
    var list = [];
    for (var i = 0; i < nodes.length; i++) {
      if (nodes[i].offsetParent !== null && !nodes[i].hidden) {
        list.push(nodes[i]);
      }
    }
    return list;
  }

  function openSheet(mode) {
    if (!state.sheetMode) {
      lastFocusedBeforeSheet = document.activeElement;
    }
    state.sheetMode = mode;
    renderSheetContent();
    elSheetBackdrop.hidden = false;
    elBottomSheet.hidden = false;
    elViewTabs.inert = true;
    elViewDetail.inert = true;

    window.requestAnimationFrame(function () {
      var focusables = getFocusableInSheet();
      if (focusables.length > 0) {
        focusables[0].focus();
      } else {
        elBtnCloseSheet.focus();
      }
    });
  }

  function closeSheet() {
    if (!state.sheetMode) return;
    state.sheetMode = null;
    elSheetBackdrop.hidden = true;
    elBottomSheet.hidden = true;
    elViewTabs.inert = false;
    elViewDetail.inert = false;
    elBtnSheetBack.hidden = true;
    if (lastFocusedBeforeSheet && typeof lastFocusedBeforeSheet.focus === 'function') {
      lastFocusedBeforeSheet.focus();
    }
    lastFocusedBeforeSheet = null;
  }

  function renderSessionRowForSheet(session, currentId, options) {
    options = options || {};
    var isCurrent = session.id === currentId;
    var subInfo = options.subText || getStatusLabel(session);
    return (
      '<button class="sheet-session-btn' +
      (isCurrent ? ' is-current' : '') +
      '" type="button" data-sheet-session="' +
      escapeHtml(session.id) +
      '">' +
      '<div class="sheet-session-main">' +
      '<div class="sheet-session-top">' +
      '<span class="sheet-session-name">' +
      escapeHtml(session.title) +
      '</span>' +
      (isCurrent ? '<span class="current-tag">当前</span>' : '') +
      '</div>' +
      '<div class="sheet-session-sub">' +
      escapeHtml(subInfo) +
      '</div>' +
      '</div>' +
      '<span class="status-badge ' +
      getStatusClass(session) +
      '">' +
      escapeHtml(getStatusLabel(session)) +
      '</span>' +
      '</button>'
    );
  }

  function renderSwitcherSheet(current) {
    elBtnSheetBack.hidden = true;
    elSheetTitle.textContent = '切换会话';
    elSheetSubtitle.textContent = formatPath(current);

    var branchNodes = getBranchTreeNodes(current.id);
    var branchIds = {};
    branchNodes.forEach(function (n) {
      branchIds[n.session.id] = true;
    });

    // 1. Current branch tree (shallow indentation)
    var branchHtml = branchNodes
      .map(function (node) {
        var guides = '';
        if (node.depth > 0) {
          var steps = '';
          for (var d = 0; d < node.depth; d++) {
            steps += '<span class="tree-step-guide" aria-hidden="true"></span>';
          }
          guides = '<div class="tree-indent">' + steps + '</div>';
        }
        var sub = node.session.branchNote || (node.depth === 0 ? '分支起点 · ' + node.session.updatedAt : '独立工作副本');
        return '<div class="tree-row">' + guides + renderSessionRowForSheet(node.session, current.id, { subText: sub }) + '</div>';
      })
      .join('');

    // 2. Same-canvas other sessions (NO inheritance line)
    var sameCanvasOthers = state.sessions.filter(function (s) {
      return (
        !branchIds[s.id] &&
        s.workspace === current.workspace &&
        s.item === current.item &&
        s.canvas === current.canvas
      );
    });

    var sameCanvasHtml = '';
    if (sameCanvasOthers.length > 0) {
      sameCanvasHtml =
        '<div>' +
        '<h4 class="sheet-section-title">同画布其他会话（' +
        escapeHtml(current.canvas) +
        ' · 无继承关系）</h4>' +
        '<div class="branch-tree">' +
        sameCanvasOthers
          .map(function (s) {
            return renderSessionRowForSheet(s, current.id, { subText: s.summary });
          })
          .join('') +
        '</div>' +
        '</div>';
    } else {
      sameCanvasHtml =
        '<div>' +
        '<h4 class="sheet-section-title">同画布其他会话</h4>' +
        '<p class="sheet-subtitle">画布「' +
        escapeHtml(current.canvas) +
        '」下暂无其他会话</p>' +
        '</div>';
    }

    // 3. Other canvases in same item (collapsible)
    var otherCanvasMap = {};
    var otherCanvasNames = [];
    state.sessions.forEach(function (s) {
      if (s.workspace === current.workspace && s.item === current.item && s.canvas !== current.canvas) {
        if (!otherCanvasMap[s.canvas]) {
          otherCanvasMap[s.canvas] = [];
          otherCanvasNames.push(s.canvas);
        }
        otherCanvasMap[s.canvas].push(s);
      }
    });

    var otherCanvasHtml = '';
    if (otherCanvasNames.length > 0) {
      var accordions = otherCanvasNames
        .map(function (cvName) {
          var key = current.workspace + '::' + current.item + '::' + cvName;
          var expanded = !!state.sheetAccordionExpanded[key];
          var rows = otherCanvasMap[cvName]
            .map(function (s) {
              return renderSessionRowForSheet(s, current.id, { subText: s.summary });
            })
            .join('');
          return (
            '<div class="canvas-accordion">' +
            '<button class="canvas-accordion-btn" type="button" data-toggle-canvas="' +
            escapeHtml(key) +
            '" aria-expanded="' +
            (expanded ? 'true' : 'false') +
            '">' +
            '<span>画布：' +
            escapeHtml(cvName) +
            '（' +
            otherCanvasMap[cvName].length +
            '）</span>' +
            '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="' +
            (expanded ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6') +
            '"></path></svg>' +
            '</button>' +
            (expanded ? '<div class="canvas-accordion-body">' + rows + '</div>' : '') +
            '</div>'
          );
        })
        .join('');

      otherCanvasHtml =
        '<div>' +
        '<h4 class="sheet-section-title">同事项其他画布（' +
        escapeHtml(current.item) +
        '）</h4>' +
        accordions +
        '</div>';
    }

    // 4. View full relationship tree or jump to location
    var footerActionHtml = '';
    var itemSessionCount = state.sessions.filter(function (s) {
      return s.workspace === current.workspace && s.item === current.item;
    }).length;
    if (itemSessionCount > 1 || otherCanvasNames.length > 0) {
      footerActionHtml =
        '<button class="sheet-footer-link" type="button" data-sheet-action="open-full-tree">' +
        '<span>查看完整关系（按画布分组的「' +
        escapeHtml(current.item) +
        '」树）</span>' +
        '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M9 18l6-6-6-6"></path></svg>' +
        '</button>';
    } else {
      footerActionHtml =
        '<button class="sheet-footer-link" type="button" data-sheet-action="open-location">' +
        '<span>查看全部工作空间与事项位置</span>' +
        '<svg class="icon" viewBox="0 0 24 24" aria-hidden="true"><path d="M9 18l6-6-6-6"></path></svg>' +
        '</button>';
    }

    elSheetBody.innerHTML =
      '<div>' +
      '<h4 class="sheet-section-title">当前分支树</h4>' +
      '<div class="branch-tree">' +
      branchHtml +
      '</div>' +
      '</div>' +
      sameCanvasHtml +
      otherCanvasHtml +
      footerActionHtml;
  }

  function renderFullTreeSheet(current) {
    elBtnSheetBack.hidden = false;
    elSheetTitle.textContent = '完整关系 · ' + current.item;
    elSheetSubtitle.textContent = '按画布分组的事项结构（点击任意会话直达）';

    var itemSessions = state.sessions.filter(function (s) {
      return s.workspace === current.workspace && s.item === current.item;
    });

    var canvasMap = {};
    var canvasNames = [];
    itemSessions.forEach(function (s) {
      if (!canvasMap[s.canvas]) {
        canvasMap[s.canvas] = [];
        canvasNames.push(s.canvas);
      }
      canvasMap[s.canvas].push(s);
    });

    var html = canvasNames
      .map(function (cvName) {
        var list = canvasMap[cvName];
        // Build forest inside this canvas
        var rendered = {};
        var rowsHtml = '';

        function renderSubtree(node, depth) {
          if (rendered[node.id]) return;
          rendered[node.id] = true;
          var guides = '';
          if (depth > 0) {
            var steps = '';
            for (var d = 0; d < depth; d++) {
              steps += '<span class="tree-step-guide" aria-hidden="true"></span>';
            }
            guides = '<div class="tree-indent">' + steps + '</div>';
          }
          rowsHtml +=
            '<div class="tree-row">' +
            guides +
            renderSessionRowForSheet(node, current.id, {
              subText: node.parentId ? '分支自 ' + (getSessionById(node.parentId) || {}).title : '画布独立会话'
            }) +
            '</div>';
          list.forEach(function (child) {
            if (child.parentId === node.id) {
              renderSubtree(child, depth + 1);
            }
          });
        }

        list.forEach(function (s) {
          var parentInCanvas =
            s.parentId &&
            list.some(function (cand) {
              return cand.id === s.parentId;
            });
          if (!parentInCanvas) {
            renderSubtree(s, 0);
          }
        });

        return (
          '<div>' +
          '<h4 class="sheet-section-title">画布：' +
          escapeHtml(cvName) +
          (cvName === current.canvas ? '（当前画布）' : '') +
          '</h4>' +
          '<div class="branch-tree">' +
          rowsHtml +
          '</div>' +
          '</div>'
        );
      })
      .join('');

    elSheetBody.innerHTML = html;
  }

  function renderLocationSheet(current) {
    elBtnSheetBack.hidden = true;
    elSheetTitle.textContent = '归属位置';
    elSheetSubtitle.textContent = '当前：' + formatPath(current);

    var hierarchy = groupSessionsHierarchy(state.sessions);
    elSheetBody.innerHTML = hierarchy
      .map(function (ws) {
        var itemsHtml = ws.items
          .map(function (it) {
            var canvasesHtml = it.canvases
              .map(function (cv) {
                var rowsHtml = cv.sessions
                  .map(function (s) {
                    return renderSessionRowForSheet(s, current.id, { subText: s.summary });
                  })
                  .join('');
                return (
                  '<div class="ws-canvas-block">' +
                  '<div class="ws-canvas-label">画布：' +
                  escapeHtml(cv.name) +
                  '</div>' +
                  '<div class="branch-tree">' +
                  rowsHtml +
                  '</div>' +
                  '</div>'
                );
              })
              .join('');

            return (
              '<div class="ws-item-block">' +
              '<div class="ws-item-title">事项：' +
              escapeHtml(it.name) +
              '</div>' +
              canvasesHtml +
              '</div>'
            );
          })
          .join('');

        return (
          '<section class="ws-group">' +
          '<div class="ws-group-header">' +
          '<div class="ws-group-title">' +
          escapeHtml(ws.name) +
          '</div>' +
          '<span class="ws-group-count">' +
          ws.count +
          ' 个会话</span>' +
          '</div>' +
          itemsHtml +
          '</section>'
        );
      })
      .join('');
  }

  function renderNewSessionSheet() {
    elBtnSheetBack.hidden = true;
    elSheetTitle.textContent = '新建本地演示会话';
    elSheetSubtitle.textContent = '仅用于体验，不会启动真实任务';

    var parentOptions = '<option value="">无父分支（独立会话）</option>' +
      state.sessions
        .map(function (s) {
          return '<option value="' + escapeHtml(s.id) + '">' + escapeHtml(s.title) + '</option>';
        })
        .join('');

    elSheetBody.innerHTML =
      '<form id="form-new-session" class="form-stack">' +
      '<div class="form-field">' +
      '<label for="new-title">会话名称 *</label>' +
      '<input id="new-title" name="title" type="text" required placeholder="例如：验证码防刷策略" />' +
      '</div>' +
      '<div class="form-grid-2">' +
      '<div class="form-field">' +
      '<label for="new-workspace">工作空间</label>' +
      '<input id="new-workspace" name="workspace" type="text" value="Matou" />' +
      '</div>' +
      '<div class="form-field">' +
      '<label for="new-item">事项</label>' +
      '<input id="new-item" name="item" type="text" value="登录改版" />' +
      '</div>' +
      '</div>' +
      '<div class="form-grid-2">' +
      '<div class="form-field">' +
      '<label for="new-canvas">画布</label>' +
      '<input id="new-canvas" name="canvas" type="text" value="登录流程" />' +
      '</div>' +
      '<div class="form-field">' +
      '<label for="new-status">初始状态</label>' +
      '<select id="new-status" name="status">' +
      '<option value="input">待输入</option>' +
      '<option value="confirm">待确认</option>' +
      '<option value="running">运行中</option>' +
      '</select>' +
      '</div>' +
      '</div>' +
      '<div class="form-field">' +
      '<label for="new-parent">父分支（可选）</label>' +
      '<select id="new-parent" name="parentId">' +
      parentOptions +
      '</select>' +
      '</div>' +
      '<div class="form-field">' +
      '<label for="new-prompt">首条说明（可选）</label>' +
      '<textarea id="new-prompt" name="prompt" rows="2" placeholder="描述你想在手机上继续的任务…"></textarea>' +
      '</div>' +
      '<button class="btn-submit-primary" type="submit">创建并进入会话</button>' +
      '</form>';
  }

  function renderSheetContent() {
    var current = state.currentSessionId ? getSessionById(state.currentSessionId) : null;
    if (state.sheetMode === 'switcher' && current) {
      renderSwitcherSheet(current);
    } else if (state.sheetMode === 'full-tree' && current) {
      renderFullTreeSheet(current);
    } else if (state.sheetMode === 'location' && current) {
      renderLocationSheet(current);
    } else if (state.sheetMode === 'new-session') {
      renderNewSessionSheet();
    }
  }

  // Event Handlers
  elNavBtns.forEach(function (btn) {
    btn.addEventListener('click', function () {
      switchTab(btn.getAttribute('data-tab'));
    });
  });

  elBtnToggleSearch.addEventListener('click', function () {
    state.searchOpen = !state.searchOpen;
    elSearchWrap.hidden = !state.searchOpen;
    elBtnToggleSearch.setAttribute('aria-expanded', state.searchOpen ? 'true' : 'false');
    if (state.searchOpen) {
      elSearchInput.focus();
    } else if (state.searchQuery) {
      state.searchQuery = '';
      elSearchInput.value = '';
      elBtnClearSearch.hidden = true;
      renderSessionList();
    }
  });

  elSearchInput.addEventListener('input', function () {
    state.searchQuery = elSearchInput.value;
    elBtnClearSearch.hidden = state.searchQuery.length === 0;
    renderSessionList();
  });

  elBtnClearSearch.addEventListener('click', function () {
    state.searchQuery = '';
    elSearchInput.value = '';
    elBtnClearSearch.hidden = true;
    renderSessionList();
    elSearchInput.focus();
  });

  elHomeFilters.addEventListener('click', function (e) {
    var btn = e.target.closest('.filter-chip');
    if (!btn) return;
    state.activeFilter = btn.getAttribute('data-filter');
    renderFilters();
    renderSessionList();
  });

  elBtnResetFilters.addEventListener('click', function () {
    state.activeFilter = 'recent';
    state.searchQuery = '';
    elSearchInput.value = '';
    elBtnClearSearch.hidden = true;
    renderFilters();
    renderSessionList();
  });

  elSessionList.addEventListener('click', function (e) {
    var card = e.target.closest('[data-session-id]');
    if (!card) return;
    openSession(card.getAttribute('data-session-id'));
  });

  elWorkspacesTree.addEventListener('click', function (e) {
    var row = e.target.closest('[data-session-id]');
    if (!row) return;
    openSession(row.getAttribute('data-session-id'));
  });

  elBtnNewSession.addEventListener('click', function () {
    openSheet('new-session');
  });

  elBtnResetDemo.addEventListener('click', function () {
    var fresh = buildInitialData();
    state.sessions = fresh.sessions;
    state.drafts = {};
    state.scrollPositions = {};
    state.activeFilter = 'recent';
    state.searchQuery = '';
    state.sheetAccordionExpanded = {};
    elSearchInput.value = '';
    elBtnClearSearch.hidden = true;
    try {
      window.sessionStorage.removeItem(STORAGE_KEY);
    } catch (e) {
      // Ignore
    }
    persistState();
    renderFilters();
    renderSessionList();
    renderWorkspaces();
    showToast('已重置为初始本地演示数据');
  });

  // Detail View Events
  elBtnDetailBack.addEventListener('click', goBack);
  elBtnOpenLocation.addEventListener('click', function () {
    openSheet('location');
  });
  elBtnOpenSwitcher.addEventListener('click', function () {
    openSheet('switcher');
  });
  elBtnDeviceStatus.addEventListener('click', function () {
    showToast('已连接桌面端：' + DEVICE_NAME + '（本地演示状态）');
  });

  elDetailScroll.addEventListener('scroll', function () {
    if (state.currentSessionId && !elViewDetail.hidden) {
      state.scrollPositions[state.currentSessionId] = elDetailScroll.scrollTop;
      persistState();
    }
  });

  elDetailMessages.addEventListener('click', function (e) {
    var toggleDiff = e.target.closest('[data-action="toggle-diff"]');
    if (toggleDiff) {
      var session = getSessionById(state.currentSessionId);
      if (!session) return;
      var savedTop = elDetailScroll.scrollTop;
      session.diffExpanded = !session.diffExpanded;
      persistState();
      renderDetailMessages(session);
      elDetailScroll.scrollTop = savedTop;
      return;
    }

    var attachBtn = e.target.closest('[data-action="attachment-demo"]');
    if (attachBtn) {
      showToast('仅本地演示说明：此处模拟桌面端同步的原型附件');
      return;
    }

    var jumpBtn = e.target.closest('[data-jump-session]');
    if (jumpBtn) {
      openSession(jumpBtn.getAttribute('data-jump-session'));
    }
  });

  elComposerInput.addEventListener('input', function () {
    if (state.currentSessionId) {
      state.drafts[state.currentSessionId] = elComposerInput.value;
      persistState();
    }
    autoResizeComposer();
    updateSendButtonState();
  });

  elComposerInput.addEventListener('keydown', function (e) {
    if (e.key === 'Enter' && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      if (!elBtnSendMessage.disabled) {
        elBtnSendMessage.click();
      }
    }
  });

  elBtnComposerAttach.addEventListener('click', function () {
    showToast('附件仅用于演示说明，本地原型不发起上传');
  });

  elBtnStopSession.addEventListener('click', function () {
    var session = getSessionById(state.currentSessionId);
    if (!session || session.status !== 'running') return;
    session.status = 'stopped';
    session.updatedAt = '刚刚';
    session.messages.push({
      id: 'sys-' + Date.now(),
      role: 'system',
      text: '已手动停止当前运行（本地演示）'
    });
    persistState();
    renderDetailView(session.id);
    window.requestAnimationFrame(function () {
      elDetailScroll.scrollTop = elDetailScroll.scrollHeight;
      state.scrollPositions[session.id] = elDetailScroll.scrollTop;
      persistState();
    });
    showToast('已停止「' + session.title + '」运行');
  });

  elBtnSendMessage.addEventListener('click', function () {
    var session = getSessionById(state.currentSessionId);
    if (!session) return;
    var text = elComposerInput.value.trim();
    if (!text) return;

    session.messages.push({
      id: 'u-' + Date.now(),
      role: 'user',
      sender: '我',
      text: text
    });
    session.updatedAt = '刚刚';
    session.summary = text;
    if (session.status === 'input' || session.status === 'stopped') {
      session.status = 'running';
    }

    state.drafts[session.id] = '';
    elComposerInput.value = '';
    autoResizeComposer();
    updateSendButtonState();
    persistState();

    renderDetailView(session.id);
    window.requestAnimationFrame(function () {
      elDetailScroll.scrollTop = elDetailScroll.scrollHeight;
      state.scrollPositions[session.id] = elDetailScroll.scrollTop;
      persistState();
    });

    // Local-only simulated demo reply (zero network requests)
    var targetSessionId = session.id;
    window.setTimeout(function () {
      var target = getSessionById(targetSessionId);
      if (!target) return;
      target.messages.push({
        id: 'ai-' + Date.now(),
        role: 'ai',
        sender: 'Matou 助手',
        isDemoReply: true,
        text: '已收到「' + text + '」。当前为本地模拟演示回复：已同步更新「' + target.title + '」上下文摘要，可继续在「' + DEVICE_NAME + '」或切换其他分支查看。'
      });
      if (target.status === 'running' && target.id !== 'alt' && target.id !== 'weak') {
        target.status = 'confirm';
      }
      persistState();
      if (state.currentSessionId === targetSessionId && !elViewDetail.hidden) {
        var followLatest = elDetailScroll.scrollHeight - elDetailScroll.scrollTop - elDetailScroll.clientHeight < 48;
        saveCurrentSessionSnapshot();
        renderDetailView(targetSessionId);
        window.requestAnimationFrame(function () {
          if (followLatest) elDetailScroll.scrollTop = elDetailScroll.scrollHeight;
          state.scrollPositions[targetSessionId] = elDetailScroll.scrollTop;
          persistState();
        });
      }
    }, 320);
  });

  // Sheet Events
  elSheetBackdrop.addEventListener('click', closeSheet);
  elBtnCloseSheet.addEventListener('click', closeSheet);
  elBtnSheetBack.addEventListener('click', function () {
    if (state.sheetMode === 'full-tree') {
      state.sheetMode = 'switcher';
      renderSheetContent();
    }
  });

  elSheetBody.addEventListener('click', function (e) {
    var sessionBtn = e.target.closest('[data-sheet-session]');
    if (sessionBtn) {
      var targetId = sessionBtn.getAttribute('data-sheet-session');
      if (targetId === state.currentSessionId) {
        closeSheet();
        return;
      }
      closeSheet();
      openSession(targetId);
      return;
    }

    var accordionBtn = e.target.closest('[data-toggle-canvas]');
    if (accordionBtn) {
      var key = accordionBtn.getAttribute('data-toggle-canvas');
      state.sheetAccordionExpanded[key] = !state.sheetAccordionExpanded[key];
      renderSheetContent();
      return;
    }

    var actionBtn = e.target.closest('[data-sheet-action]');
    if (actionBtn) {
      var act = actionBtn.getAttribute('data-sheet-action');
      if (act === 'open-full-tree') {
        state.sheetMode = 'full-tree';
        renderSheetContent();
      } else if (act === 'open-location') {
        state.sheetMode = 'location';
        renderSheetContent();
      }
    }
  });

  elSheetBody.addEventListener('submit', function (e) {
    if (e.target && e.target.id === 'form-new-session') {
      e.preventDefault();
      var title = (document.getElementById('new-title').value || '').trim();
      if (!title) return;

      var workspace = (document.getElementById('new-workspace').value || '').trim() || 'Matou';
      var item = (document.getElementById('new-item').value || '').trim() || '默认事项';
      var canvas = (document.getElementById('new-canvas').value || '').trim() || '默认画布';
      var status = document.getElementById('new-status').value || 'input';
      var parentId = document.getElementById('new-parent').value || null;
      var prompt = (document.getElementById('new-prompt').value || '').trim();

      var newId = 'demo-' + Date.now();
      var code = 's' + String(state.sessions.length + 1);
      var newSession = {
        id: newId,
        code: code,
        title: title,
        status: status,
        statusDetail: '',
        parentId: parentId,
        branchNote: parentId ? '继承上下文·独立工作副本' : '',
        highlight: false,
        workspace: workspace,
        item: item,
        canvas: canvas,
        updatedAt: '刚刚',
        summary: prompt || '本地新建演示会话，等待输入下一步指令',
        messages: [
          {
            id: newId + '-sys',
            role: 'system',
            text: '本地新建演示会话（仅保存于当前 sessionStorage）'
          }
        ]
      };

      if (prompt) {
        newSession.messages.push({
          id: newId + '-u1',
          role: 'user',
          sender: '我',
          text: prompt
        });
        newSession.messages.push({
          id: newId + '-ai1',
          role: 'ai',
          sender: 'Matou 助手',
          isDemoReply: true,
          text: '已建立「' + title + '」本地演示会话，归属路径：' + formatPath(newSession) + '。'
        });
      }

      state.sessions.unshift(newSession);
      persistState();
      closeSheet();
      renderFilters();
      renderSessionList();
      openSession(newId);
      showToast('已创建本地演示会话「' + title + '」');
    }
  });

  // Global keyboard handler: Escape & Focus Trap inside Bottom Sheet
  document.addEventListener('keydown', function (e) {
    if (!state.sheetMode) return;
    if (e.key === 'Escape') {
      e.preventDefault();
      closeSheet();
      return;
    }
    if (e.key === 'Tab') {
      var focusables = getFocusableInSheet();
      if (focusables.length === 0) {
        e.preventDefault();
        return;
      }
      var first = focusables[0];
      var last = focusables[focusables.length - 1];
      if (e.shiftKey) {
        if (document.activeElement === first || !elBottomSheet.contains(document.activeElement)) {
          e.preventDefault();
          last.focus();
        }
      } else {
        if (document.activeElement === last || !elBottomSheet.contains(document.activeElement)) {
          e.preventDefault();
          first.focus();
        }
      }
    }
  });

  // Initialize
  renderFilters();
  renderSessionList();
  renderWorkspaces();
})();
