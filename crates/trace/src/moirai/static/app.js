let currentTraceId = null;
let currentSpan = null;
let currentSpans = [];
let autoRefreshInterval = null;
let isAutoRefreshing = false;

// ============ View Management ============

function showView(viewId) {
    ['trace-list', 'span-tree', 'timeline', 'detail-panel'].forEach(id => {
        document.getElementById(id).classList.add('hidden');
    });
    document.getElementById(viewId).classList.remove('hidden');
    
    ['btn-trace-list', 'btn-trace-detail'].forEach(id => {
        const btn = document.getElementById(id);
        if (btn) btn.classList.remove('bg-blue-600');
    });
    const activeBtn = document.getElementById('btn-' + viewId);
    if (activeBtn) activeBtn.classList.add('bg-blue-600');
}

function showToast() {
    const toast = document.getElementById('toast');
    toast.style.right = '16px';
    setTimeout(() => {
        toast.style.right = '-8rem';
    }, 2000);
}

// ============ Trace List ============

// 首屏/每页条数；是否还有下一页由后端返回的 next_cursor 表达。
const PAGE_SIZE = 50;

// 每次点击「加载更多」追加的条数。比 PAGE_SIZE 大一档，纯为减少深翻页
// （千条级历史）时的点击次数；请求成本对页大小不敏感。
const LOAD_MORE_PAGE = 200;

// 已加载的 trace，跨多次「加载更多」累积。
let loadedTraces = [];
// 下一页游标（后端原样返回的 "{start_time}.{trace_id}"），null 表示没有更多。
let nextCursor = null;
let isLoadingMore = false;

// trace_id -> { row: <tr>, trace: 该行最后一次渲染的快照 }。已渲染行的
// single source of truth：新行判定、原地更新、消失行清理都从它读取；
// trace 用于变更比较，深度重拉时多数行未变化，跳过 DOM 写入。
let traceRows = new Map();

// 串行化对 loadedTraces/nextCursor 的读写：自动刷新若与「加载更多」并发，
// 刷新会按旧深度截断列表，「加载更多」会向新列表拼接按过期游标取回的页，
// 两种交错都会留下重复或丢失行；排队执行则各自都基于最新状态。
let traceListOp = Promise.resolve();

function runSerializedTraceListOp(task) {
    const result = traceListOp.then(task, task);
    // 任务内部已自行捕获异常；兜底 catch 防止链路因意外 rejection 永久卡死。
    traceListOp = result.catch(() => {});
    return result;
}

function hasMoreTraces() {
    return nextCursor !== null;
}

function tracesUrl(limit, cursor) {
    const alive = document.getElementById('filter-alive').checked;
    let url = `/api/traces?limit=${limit}&alive=${alive}`;
    if (cursor) {
        url += `&cursor=${encodeURIComponent(cursor)}`;
    }
    return url;
}

async function fetchTraces(limit, cursor) {
    const response = await fetch(tracesUrl(limit, cursor));
    if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
    }
    return await response.json();
}

// keepDepth：按已加载数量请求，避免「加载更多」展开的列表在刷新后回缩。
// 深度不设上限：本地单用户规模下整表查询为毫秒级，且渲染走行级调和
// （见 reconcileTraceRows），多数行未变化时零 DOM 写入；真正撑不住的
// 规模下，先触顶的是把全部已加载行挂在 DOM 里这件事本身，而不是这次
// 重拉，封顶拦不住前者，只会挡住用户翻完整历史。
// quiet：自动刷新时网络抖动不应清掉现有列表，只打日志。
function reloadTraces({ keepDepth = false, animate = false, toast = false, quiet = false } = {}) {
    return runSerializedTraceListOp(async () => {
        try {
            const limit = Math.max(PAGE_SIZE, keepDepth ? loadedTraces.length : 0);
            const data = await fetchTraces(limit, null);
            loadedTraces = data.items;
            nextCursor = data.next_cursor;
            renderTraceList(loadedTraces, animate);
            if (toast) showToast();
        } catch (error) {
            if (quiet) {
                console.error('Auto refresh failed:', error);
                return;
            }
            // 错误态整体替换容器，map 里的行已随之脱离 DOM；
            // 在此同步清空，避免下次渲染把陈旧行重新挂回。
            traceRows.clear();
            document.getElementById('trace-list-content').innerHTML =
                `<p class="text-red-600">Error loading traces: ${error.message}</p>`;
            nextCursor = null;
        }
        updateLoadMoreButton();
    });
}

async function loadTraces() {
    await reloadTraces();
}

async function refreshTraces() {
    await reloadTraces({ keepDepth: true, toast: true });
}

function loadMore() {
    if (!hasMoreTraces() || isLoadingMore) return;
    isLoadingMore = true;
    updateLoadMoreButton();
    return runSerializedTraceListOp(async () => {
        try {
            // 排队期间状态可能已被前面的刷新任务改写，执行时再校验一次。
            if (!hasMoreTraces()) return;
            const data = await fetchTraces(LOAD_MORE_PAGE, nextCursor);
            nextCursor = data.next_cursor;
            const page = data.items;
            loadedTraces = loadedTraces.concat(page);
            appendTraceRows(page);
        } catch (error) {
            alert('Error loading more traces: ' + error.message);
        } finally {
            isLoadingMore = false;
            updateLoadMoreButton();
        }
    });
}

function updateLoadMoreButton() {
    const container = document.getElementById('load-more');
    if (!container) return;
    document.getElementById('loaded-count').textContent = loadedTraces.length;
    container.classList.toggle('hidden', !hasMoreTraces());
    const btn = document.getElementById('btn-load-more');
    if (btn) btn.disabled = isLoadingMore;
}

// 行内六个单元格的内部 HTML，新建行与变更行的原地更新共用同一份拼装。
function traceRowInnerHtml(trace) {
    const taskCell = trace.task_description
        ? `<span class="block truncate max-w-md" title="${escapeAttr(trace.task_description)}">${escapeHtml(trace.task_description)}</span>`
        : '<span class="text-gray-400">-</span>';
    return `
        <td class="border p-2 font-mono text-sm">${escapeHtml(trace.trace_id.substring(0, 12))}</td>
        <td class="border p-2">${taskCell}</td>
        <td class="border p-2">${trace.span_count}</td>
        <td class="border p-2">${formatTime(trace.start_time)}</td>
        <td class="border p-2">
            ${trace.end_time ? 
                '<span class="text-gray-600">ended</span>' : 
                '<span class="text-green-600 font-semibold">alive</span>'}
        </td>
        <td class="border p-2 text-center">
            <button data-action="delete"
                    class="text-red-600 hover:text-red-800 p-1"
                    title="Delete trace">
                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path>
                </svg>
            </button>
        </td>
    `;
}

// data-id 经 dataset 赋值，元素属性天然安全，无需手动转义。
function createTraceRow(trace, isNew) {
    const row = document.createElement('tr');
    row.className = `hover:bg-gray-100 trace-row cursor-pointer${isNew ? ' new-trace-row' : ''}`;
    row.dataset.id = trace.trace_id;
    row.innerHTML = traceRowInnerHtml(trace);
    return row;
}

// 参与比较的字段即建行后仍会变化的单元格（task/spans/started/status）；
// trace_id 建行后不变，无需比较。
function traceChanged(a, b) {
    return a.task_description !== b.task_description
        || a.span_count !== b.span_count
        || a.start_time !== b.start_time
        || a.end_time !== b.end_time;
}

function traceTableHtml(rowsHtml) {
    return `
        <table class="w-full border-collapse">
            <thead>
                <tr class="bg-gray-200">
                    <th class="border p-2 text-left">Trace ID</th>
                    <th class="border p-2 text-left">Task</th>
                    <th class="border p-2 text-left">Spans</th>
                    <th class="border p-2 text-left">Started</th>
                    <th class="border p-2 text-left">Status</th>
                    <th class="border p-2 text-left w-20">Actions</th>
                </tr>
            </thead>
            <tbody>
                ${rowsHtml}
            </tbody>
        </table>
    `;
}

// 「加载更多」只往 tbody 末尾追加行，避免全量重绘丢滚动位置。
function appendTraceRows(newTraces) {
    const tbody = document.querySelector('#trace-list-content tbody');
    if (!tbody) return;
    for (const trace of newTraces) {
        const row = createTraceRow(trace, false);
        tbody.appendChild(row);
        traceRows.set(trace.trace_id, { row, trace });
    }
}

// 表格骨架与空态提示只创建一次，此后渲染只做行级调和，
// 不再每次整表 innerHTML 重解析。
function ensureTraceDom() {
    const container = document.getElementById('trace-list-content');
    if (!container.querySelector('table')) {
        container.innerHTML = `
            <p id="trace-empty" class="text-gray-600 hidden">No traces found</p>
            ${traceTableHtml('')}
        `;
    }
    return container;
}

// 行级调和：服务端顺序即 keyset 分页的全序，旧行按 id 原地更新、新行按
// 位插入、消失行移除。ref 指向当前行应落位其前的元素，双指针一遍走完
// 插入与删除后的重排。
function reconcileTraceRows(tbody, traces, newIds) {
    const incomingIds = new Set(traces.map(t => t.trace_id));

    let ref = tbody.firstElementChild;
    for (const trace of traces) {
        const entry = traceRows.get(trace.trace_id);
        let row;
        if (entry) {
            row = entry.row;
            // 先比较再覆写快照：多数行未变化时零 DOM 写入
            if (traceChanged(entry.trace, trace)) {
                row.innerHTML = traceRowInnerHtml(trace);
            }
            entry.trace = trace;
        } else {
            row = createTraceRow(trace, newIds.has(trace.trace_id));
            traceRows.set(trace.trace_id, { row, trace });
        }
        if (row !== ref) {
            tbody.insertBefore(row, ref);
        }
        ref = row.nextElementSibling;
    }

    // Map 迭代中删除已访问项是规范保证安全的
    for (const [id, entry] of traceRows) {
        if (!incomingIds.has(id)) {
            entry.row.remove();
            traceRows.delete(id);
        }
    }
}

// FLIP Last/Invert/Play：批量读出新位置、批量写入反向变换、再于嵌套
// rAF 批量清除。各相内读写不交错，整轮至多 2 次强制 layout 刷新，
// 而非逐行各刷一次。
function applyFlip(firstPositions) {
    if (firstPositions.size === 0) return;
    requestAnimationFrame(() => {
        const moves = [];
        for (const [id, entry] of traceRows) {
            const firstTop = firstPositions.get(id);
            // 不在 firstPositions 里的必是新行，已有 bounceIn 入场动画
            if (firstTop === undefined) continue;
            const delta = firstTop - entry.row.getBoundingClientRect().top;
            if (Math.abs(delta) > 1) {
                moves.push({ row: entry.row, delta });
            }
        }
        if (moves.length === 0) return;
        for (const { row, delta } of moves) {
            row.style.transform = `translateY(${delta}px)`;
            row.style.transition = 'none';
        }
        requestAnimationFrame(() => {
            for (const { row } of moves) {
                row.style.transition = 'transform 0.4s cubic-bezier(0.25, 0.46, 0.45, 0.94)';
                row.style.transform = '';
            }
        });
    });
}

function renderTraceList(traces, animate = false) {
    const container = ensureTraceDom();
    const table = container.querySelector('table');
    const emptyMsg = container.querySelector('#trace-empty');
    const tbody = container.querySelector('tbody');

    const isEmpty = traces.length === 0;
    table.classList.toggle('hidden', isEmpty);
    emptyMsg.classList.toggle('hidden', !isEmpty);
    if (isEmpty) {
        for (const entry of traceRows.values()) {
            entry.row.remove();
        }
        traceRows.clear();
        return;
    }

    // FLIP First：调和前批量读旧位置；之后的写集中在 reconcileTraceRows，
    // 读写全程不交错。
    const firstPositions = new Map();
    if (animate) {
        for (const [id, entry] of traceRows) {
            firstPositions.set(id, entry.row.getBoundingClientRect().top);
        }
    }

    const prevIds = new Set(traceRows.keys());
    const newIds = animate
        ? new Set(traces.filter(t => !prevIds.has(t.trace_id)).map(t => t.trace_id))
        : new Set();
    reconcileTraceRows(tbody, traces, newIds);
    applyFlip(firstPositions);
}

// ============ Trace Detail ============

async function viewTrace(traceId) {
    currentTraceId = traceId;
    showView('span-tree');
    await loadTraceDetail(traceId);
}

async function loadTraceDetail(traceId) {
    document.getElementById('timeline-content').innerHTML = '<p class="text-gray-600">Loading...</p>';
    document.getElementById('detail-content').innerHTML = '<p class="text-gray-500">Click a span in timeline to view details</p>';
    
    try {
        const response = await fetch(`/api/traces/${traceId.substring(0, 12)}`);
        currentSpans = await response.json();
        renderTimeline(currentSpans);
    } catch (error) {
        document.getElementById('timeline-content').innerHTML = 
            `<p class="text-red-600">Error loading trace: ${error.message}</p>`;
    }
}

// ============ Delete ============

let traceToDelete = null;

function confirmDelete(traceId) {
    traceToDelete = traceId;
    document.getElementById('delete-modal').classList.remove('hidden');
    document.getElementById('delete-modal').classList.add('flex');
}

function hideDeleteModal() {
    document.getElementById('delete-modal').classList.add('hidden');
    document.getElementById('delete-modal').classList.remove('flex');
    traceToDelete = null;
}

async function deleteTrace() {
    if (!traceToDelete) return;
    
    try {
        const response = await fetch(`/api/traces/${traceToDelete.substring(0, 12)}`, {
            method: 'DELETE'
        });
        
        if (!response.ok) {
            throw new Error('Delete failed');
        }
        
        hideDeleteModal();
        loadTraces();
    } catch (error) {
        alert('Error deleting trace: ' + error.message);
    }
}

// ============ Utilities ============

function formatTime(timestamp) {
    const date = new Date(timestamp);
    return date.toLocaleString();
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

// escapeHtml leaves quotes intact; escape them for attribute contexts.
function escapeAttr(text) {
    return escapeHtml(text).replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

function getSpanTypeColor(type) {
    const colors = {
        'USER': 'bg-blue-200 text-blue-800',
        'QUEST': 'bg-purple-200 text-purple-800',
        'SPAWNED': 'bg-cyan-200 text-cyan-800',
        'THINK': 'bg-indigo-200 text-indigo-800',
        'TOOL': 'bg-green-200 text-green-800',
        'TOOL_CALL': 'bg-green-200 text-green-800',
        'AgentCOMM': 'bg-teal-200 text-teal-800',
        'SPAWN': 'bg-yellow-200 text-yellow-800',
        'MERGE': 'bg-orange-200 text-orange-800',
        'PUBLISH': 'bg-pink-200 text-pink-800',
        'VERIFY': 'bg-lime-200 text-lime-800',
        'ALERT': 'bg-red-200 text-red-800',
        'ERR': 'bg-red-300 text-red-900',
        'END': 'bg-gray-200 text-gray-800',
        'TURN': 'bg-blue-200 text-blue-800',
        'COMPRESSION': 'bg-yellow-200 text-yellow-800',
        'PROMPT_BUILD': 'bg-purple-200 text-purple-800',
        'HOOK': 'bg-orange-200 text-orange-800'
    };
    return colors[type] || 'bg-gray-200 text-gray-800';
}

// ============ Auto Refresh ============

function toggleAutoRefresh() {
    const btn = document.getElementById('btn-auto-refresh');
    const icon = document.getElementById('refresh-icon');
    const label = document.getElementById('refresh-label');
    
    if (isAutoRefreshing) {
        stopAutoRefresh();
        btn.classList.remove('bg-blue-600', 'text-white');
        btn.classList.add('text-gray-400');
        icon.style.animation = '';
        label.textContent = 'Auto';
    } else {
        startAutoRefresh();
        btn.classList.add('bg-blue-600', 'text-white');
        btn.classList.remove('text-gray-400');
        icon.style.animation = 'spin 1s linear infinite';
        label.textContent = 'Stop';
    }
}

function startAutoRefresh() {
    isAutoRefreshing = true;
    doAutoRefresh();
    autoRefreshInterval = setInterval(doAutoRefresh, 3000);
}

function stopAutoRefresh() {
    isAutoRefreshing = false;
    if (autoRefreshInterval) {
        clearInterval(autoRefreshInterval);
        autoRefreshInterval = null;
    }
    if (window.timelineSetAutoRefresh) {
        window.timelineSetAutoRefresh(false);
    }
}

async function doAutoRefresh() {
    const traceListVisible = !document.getElementById('trace-list').classList.contains('hidden');
    const spanTreeVisible = !document.getElementById('span-tree').classList.contains('hidden');
    
    if (traceListVisible) {
        await reloadTraces({ keepDepth: true, animate: true, quiet: true });
    } else if (spanTreeVisible && currentTraceId) {
        try {
            const response = await fetch(`/api/traces/${currentTraceId.substring(0, 12)}`);
            currentSpans = await response.json();
            renderTimeline(currentSpans, true);
            if (window.timelineSetAutoRefresh) {
                window.timelineSetAutoRefresh(true);
            }
        } catch (error) {
            console.error('Auto refresh failed:', error);
        }
    }
}

// ============ Event Listeners ============

document.addEventListener('DOMContentLoaded', () => {
    document.getElementById('btn-trace-list').addEventListener('click', () => {
        showView('trace-list');
        loadTraces();
    });
    document.getElementById('btn-trace-detail').addEventListener('click', () => {
        if (currentTraceId) showView('span-tree');
    });
    document.getElementById('btn-refresh').addEventListener('click', refreshTraces);
    document.getElementById('btn-refresh-detail').addEventListener('click', async () => {
        if (currentTraceId) {
            await loadTraceDetail(currentTraceId);
            showToast();
        }
    });
    document.getElementById('filter-alive').addEventListener('change', loadTraces);
    document.getElementById('btn-back').addEventListener('click', () => {
        showView('trace-list');
        loadTraces();
    });
    document.getElementById('btn-cancel-delete').addEventListener('click', hideDeleteModal);
    document.getElementById('btn-confirm-delete').addEventListener('click', deleteTrace);
    document.getElementById('btn-auto-refresh').addEventListener('click', toggleAutoRefresh);
    document.getElementById('btn-load-more').addEventListener('click', loadMore);

    // 行由渲染动态创建与销毁，内联 onclick 会随之丢失，且内插 trace_id
    // 有注入风险；统一委托给常驻容器，trace_id 从 data-id 读取
    //（dataset 赋值天然安全）。
    document.getElementById('trace-list-content').addEventListener('click', (event) => {
        const row = event.target.closest('tr[data-id]');
        if (!row) return;
        const traceId = row.dataset.id;
        if (event.target.closest('button[data-action="delete"]')) {
            confirmDelete(traceId);
        } else {
            viewTrace(traceId);
        }
    });

    loadTraces();
});
