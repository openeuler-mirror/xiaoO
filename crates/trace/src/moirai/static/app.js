let currentTraceId = null;
let currentSpan = null;
let currentSpans = [];
let currentTraceIds = [];
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

async function loadTraces() {
    const alive = document.getElementById('filter-alive').checked;
    const url = `/api/traces?limit=50&alive=${alive}`;
    
    try {
        const response = await fetch(url);
        const traces = await response.json();
        renderTraceList(traces);
    } catch (error) {
        document.getElementById('trace-list-content').innerHTML = 
            `<p class="text-red-600">Error loading traces: ${error.message}</p>`;
    }
}

async function refreshTraces() {
    const alive = document.getElementById('filter-alive').checked;
    const url = `/api/traces?limit=50&alive=${alive}`;
    
    try {
        const response = await fetch(url);
        const traces = await response.json();
        renderTraceList(traces);
        showToast();
    } catch (error) {
        document.getElementById('trace-list-content').innerHTML = 
            `<p class="text-red-600">Error loading traces: ${error.message}</p>`;
    }
}

function renderTraceList(traces, animate = false) {
    const container = document.getElementById('trace-list-content');
    
    if (traces.length === 0) {
        container.innerHTML = '<p class="text-gray-600">No traces found</p>';
        currentTraceIds = [];
        return;
    }

    // FLIP 动画：记录旧位置
    let firstPositions = new Map();
    if (animate && container.querySelector('tbody')) {
        const rows = container.querySelectorAll('tr[data-id]');
        rows.forEach(row => {
            const rect = row.getBoundingClientRect();
            firstPositions.set(row.dataset.id, { top: rect.top, left: rect.left });
        });
    }

    const newTraceIds = traces.map(t => t.trace_id);
    const newIds = animate ? newTraceIds.filter(id => !currentTraceIds.includes(id)) : [];
    
    currentTraceIds = newTraceIds;

    const table = `
        <table class="w-full border-collapse">
            <thead>
                <tr class="bg-gray-200">
                    <th class="border p-2 text-left">Trace ID</th>
                    <th class="border p-2 text-left">Spans</th>
                    <th class="border p-2 text-left">Started</th>
                    <th class="border p-2 text-left">Status</th>
                    <th class="border p-2 text-left w-20">Actions</th>
                </tr>
            </thead>
            <tbody>
                ${traces.map((trace, idx) => {
                    const isNew = newIds.includes(trace.trace_id);
                    return `
                    <tr class="hover:bg-gray-100 trace-row ${isNew ? 'new-trace-row' : ''}" data-id="${trace.trace_id}">
                        <td class="border p-2 font-mono text-sm cursor-pointer" onclick="viewTrace('${trace.trace_id}')">${trace.trace_id.substring(0, 12)}</td>
                        <td class="border p-2 cursor-pointer" onclick="viewTrace('${trace.trace_id}')">${trace.span_count}</td>
                        <td class="border p-2 cursor-pointer" onclick="viewTrace('${trace.trace_id}')">${formatTime(trace.start_time)}</td>
                        <td class="border p-2 cursor-pointer" onclick="viewTrace('${trace.trace_id}')">
                            ${trace.end_time ? 
                                '<span class="text-gray-600">ended</span>' : 
                                '<span class="text-green-600 font-semibold">alive</span>'}
                        </td>
                        <td class="border p-2 text-center">
                            <button onclick="confirmDelete(event, '${trace.trace_id}')" 
                                    class="text-red-600 hover:text-red-800 p-1" 
                                    title="Delete trace">
                                <svg class="w-5 h-5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"></path>
                                </svg>
                            </button>
                        </td>
                    </tr>
                `}).join('')}
            </tbody>
        </table>
    `;
    container.innerHTML = table;

    // FLIP 动画：应用位置变换
    if (animate && firstPositions.size > 0) {
        requestAnimationFrame(() => {
            const rows = container.querySelectorAll('tr[data-id]');
            rows.forEach(row => {
                const id = row.dataset.id;
                const first = firstPositions.get(id);
                if (!first) return; // 新行已有 bounceIn 动画
                
                const last = row.getBoundingClientRect();
                const deltaY = first.top - last.top;
                
                if (Math.abs(deltaY) > 1) {
                    // Invert: 反转到旧位置
                    row.style.transform = `translateY(${deltaY}px)`;
                    row.style.transition = 'none';
                    
                    // Play: 播放动画
                    requestAnimationFrame(() => {
                        row.style.transition = 'transform 0.4s cubic-bezier(0.25, 0.46, 0.45, 0.94)';
                        row.style.transform = '';
                    });
                }
            });
        });
    }
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

function confirmDelete(event, traceId) {
    event.stopPropagation();
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

function getSpanTypeColor(type) {
    const colors = {
        'USER': 'bg-blue-200 text-blue-800',
        'QUEST': 'bg-purple-200 text-purple-800',
        'SPAWNED': 'bg-cyan-200 text-cyan-800',
        'THINK': 'bg-indigo-200 text-indigo-800',
        'TOOL': 'bg-green-200 text-green-800',
        'AgentCOMM': 'bg-teal-200 text-teal-800',
        'SPAWN': 'bg-yellow-200 text-yellow-800',
        'MERGE': 'bg-orange-200 text-orange-800',
        'PUBLISH': 'bg-pink-200 text-pink-800',
        'VERIFY': 'bg-lime-200 text-lime-800',
        'ALERT': 'bg-red-200 text-red-800',
        'ERR': 'bg-red-300 text-red-900',
        'END': 'bg-gray-200 text-gray-800'
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
        const alive = document.getElementById('filter-alive').checked;
        const url = `/api/traces?limit=50&alive=${alive}`;
        try {
            const response = await fetch(url);
            const traces = await response.json();
            renderTraceList(traces, true);
        } catch (error) {
            console.error('Auto refresh failed:', error);
        }
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

    loadTraces();
});
