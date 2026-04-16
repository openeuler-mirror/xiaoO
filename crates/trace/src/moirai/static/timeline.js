const SPAN_COLORS = {
    'USER': { bg: '#bfdbfe', border: '#3b82f6', hover: '#93c5fd', text: '#1e40af' },
    'QUEST': { bg: '#e9d5ff', border: '#a855f7', hover: '#d8b4fe', text: '#6b21a8' },
    'SPAWNED': { bg: '#cffafe', border: '#06b6d4', hover: '#a5f3fc', text: '#0e7490' },
    'THINK': { bg: '#c7d2fe', border: '#6366f1', hover: '#a5b4fc', text: '#4338ca' },
    'LLM_CALL': { bg: '#c7d2fe', border: '#6366f1', hover: '#a5b4fc', text: '#4338ca' },
    'TOOL': { bg: '#bbf7d0', border: '#22c55e', hover: '#86efac', text: '#166534' },
    'TOOL_CALL': { bg: '#bbf7d0', border: '#22c55e', hover: '#86efac', text: '#166534' },
    'AgentCOMM': { bg: '#99f6e4', border: '#14b8a6', hover: '#5eead4', text: '#115e59' },
    'SPAWN': { bg: '#fef08a', border: '#eab308', hover: '#fde047', text: '#a16207' },
    'MERGE': { bg: '#fed7aa', border: '#f97316', hover: '#fdba74', text: '#c2410c' },
    'PUBLISH': { bg: '#fbcfe8', border: '#ec4899', hover: '#f9a8d4', text: '#9d174d' },
    'VERIFY': { bg: '#d9f99d', border: '#84cc16', hover: '#bef264', text: '#3f6212' },
    'ALERT': { bg: '#fecaca', border: '#ef4444', hover: '#fca5a5', text: '#991b1b' },
    'ERR': { bg: '#fca5a5', border: '#dc2626', hover: '#f87171', text: '#7f1d1d' },
    'END': { bg: '#e5e7eb', border: '#6b7280', hover: '#d1d5db', text: '#374151' },
    'TURN': { bg: '#dbeafe', border: '#3b82f6', hover: '#bfdbfe', text: '#1e40af' },
    'COMPRESSION': { bg: '#fef9c3', border: '#ca8a04', hover: '#fef08a', text: '#854d0e' },
    'PROMPT_BUILD': { bg: '#f3e8ff', border: '#9333ea', hover: '#e9d5ff', text: '#6b21a8' },
    'HOOK': { bg: '#ffedd5', border: '#ea580c', hover: '#fed7aa', text: '#9a3412' }
};

function easeOutCubic(t) {
    return 1 - Math.pow(1 - t, 3);
}

function easeOutBack(t) {
    const c1 = 1.70158;
    const c3 = c1 + 1;
    return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
}

let timelineAnimationState = {
    prevSpans: [],
    animationProgress: 1,
    animationStartTime: 0,
    ongoingAnimationId: null
};

const SPAN_TYPE_ALIASES = {
    'LLM_CALL': 'THINK',
    'TOOL_CALL': 'TOOL'
};

// Fields added by the backend to every span — redundant with span header, hide from Other Info
const GLOBAL_NOISE_KEYS = new Set(['name', 'trace_id', 'parent_span_id']);

const DEDICATED_DETAIL_KEYS = {
    'THINK': ['input_preview', 'effective_request', 'output_preview', 'final_response'],
    'TOOL': ['tool_name', 'input', 'effective_input', 'output', 'final_output', 'stdout', 'stderr', 'success', 'result_kind'],
    'TURN': ['turn_number', 'agent_id', 'prompt_tokens', 'completion_tokens', 'total_tokens', 'has_tool_calls', 'stop_reason', 'outcome'],
    'COMPRESSION': ['turn_number', 'agent_id', 'message_count', 'estimated_tokens', 'available_tokens', 'usage_ratio', 'severity', 'needs_compression', 'skipped', 'messages_before', 'messages_after', 'removed_count', 'has_summary', 'estimated_tokens_after', 'error', 'outcome'],
    'PROMPT_BUILD': ['turn_number', 'agent_id', 'message_count', 'visible_tool_count', 'skill_count', 'has_system_prompt', 'estimated_input_tokens', 'request_message_count', 'error', 'outcome'],
    'HOOK': ['hook_kind', 'hooker_id', 'hook_point', 'tool_name', 'call_id', 'result', 'error', 'outcome'],
};

function getCompatibleSpanType(spanType) {
    return SPAN_TYPE_ALIASES[spanType] || spanType;
}

function getAliasedExtraValue(extras, keys) {
    if (!extras) {
        return undefined;
    }

    for (const key of keys) {
        if (extras[key] !== undefined) {
            return extras[key];
        }
    }

    return undefined;
}

function formatExtraValue(value) {
    if (value === undefined || value === null) {
        return null;
    }

    return typeof value === 'object' ? JSON.stringify(value, null, 2) : String(value);
}

function createCollapsiblePanel(id, title, content, bgColor, borderColor, textColor = '', defaultCollapsed = false) {
    const collapsedClass = defaultCollapsed ? 'collapsed' : '';
    return `
        <div class="mb-4">
            <div class="collapsible-panel ${bgColor} border ${borderColor}">
                <div class="collapsible-header" onclick="toggleCollapsible('${id}')">
                    <label class="font-semibold text-gray-700 cursor-pointer">${title}</label>
                    <svg class="w-5 h-5 text-gray-500 collapsible-arrow ${collapsedClass}" id="arrow-${id}" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7"></path>
                    </svg>
                </div>
                <div class="collapsible-content ${collapsedClass}" id="content-${id}">
                    <div class="overflow-auto p-4" style="resize: vertical; min-height: 80px; max-height: 80vh;">
                        <pre class="whitespace-pre-wrap text-sm font-mono ${textColor}">${escapeHtml(content)}</pre>
                    </div>
                </div>
            </div>
        </div>
    `;
}

function toggleCollapsible(id) {
    const content = document.getElementById('content-' + id);
    const arrow = document.getElementById('arrow-' + id);
    if (content && arrow) {
        content.classList.toggle('collapsed');
        arrow.classList.toggle('collapsed');
    }
}

function getToolStatus(extras) {
    if (!extras) {
        return null;
    }

    if (typeof extras.success === 'boolean') {
        return extras.success
            ? { text: '✓ Success', className: 'text-green-600 bg-green-100', success: true }
            : { text: '✗ Failed', className: 'text-red-600 bg-red-100', success: false };
    }

    if (typeof extras.result_kind === 'string' && extras.result_kind !== '') {
        if (extras.result_kind === 'success') {
            return { text: '✓ Success', className: 'text-green-600 bg-green-100', success: true };
        }

        return {
            text: extras.result_kind,
            className: 'text-red-600 bg-red-100',
            success: false
        };
    }

    return null;
}

function getSpanDetailCompatibility(span) {
    const extras = span.extras || null;
    const compatibleType = getCompatibleSpanType(span.span_type);
    const dedicatedKeys = DEDICATED_DETAIL_KEYS[compatibleType] || [];
    // promoted = type-specific keys + global noise — all hidden from "Other Info"
    const promotedKeys = new Set([...dedicatedKeys, ...GLOBAL_NOISE_KEYS]);

    return {
        compatibleType,
        extras,
        promotedKeys,
        llmInput: getAliasedExtraValue(extras, ['effective_request', 'input_preview']),
        llmOutput: getAliasedExtraValue(extras, ['final_response', 'output_preview']),
        toolName: getAliasedExtraValue(extras, ['tool_name']),
        toolInput: getAliasedExtraValue(extras, ['effective_input', 'input']),
        toolOutput: getAliasedExtraValue(extras, ['final_output', 'output']),
        toolStdout: getAliasedExtraValue(extras, ['stdout']),
        toolStderr: getAliasedExtraValue(extras, ['stderr']),
        toolStatus: getToolStatus(extras)
    };
}

// ============ Structured Span Detail Renderers ============

const ROLE_BADGE = {
    'system':    'bg-gray-200 text-gray-700',
    'user':      'bg-blue-200 text-blue-800',
    'assistant': 'bg-green-200 text-green-800',
    'tool':      'bg-yellow-200 text-yellow-800',
};
const ROLE_BORDER = {
    'system':    'border-l-4 border-gray-300 bg-gray-50',
    'user':      'border-l-4 border-blue-300 bg-blue-50',
    'assistant': 'border-l-4 border-green-300 bg-green-50',
    'tool':      'border-l-4 border-yellow-300 bg-yellow-50',
};

function renderContentBlockHtml(block) {
    if (!block || typeof block !== 'object') return '';
    switch (block.type) {
        case 'text':
            return `<pre class="whitespace-pre-wrap text-sm font-mono break-words m-0">${escapeHtml(block.text || '')}</pre>`;
        case 'tool_use':
            return `<div class="my-1 bg-green-50 border border-green-200 rounded p-2">
                <div class="flex items-center gap-2 mb-1">
                    <span class="font-semibold text-green-700 text-xs font-mono">${escapeHtml(block.tool_name || '')}</span>
                    <span class="text-gray-400 text-xs">${escapeHtml(block.call_id || '')}</span>
                </div>
                <pre class="text-xs font-mono bg-white p-1 rounded overflow-auto max-h-32">${escapeHtml(JSON.stringify(block.input, null, 2))}</pre>
            </div>`;
        case 'tool_result': {
            const errCls = block.is_error ? 'border-red-200 bg-red-50' : 'border-gray-200 bg-white';
            const errLabel = block.is_error ? '<span class="text-xs text-red-500 font-semibold ml-1">error</span>' : '';
            return `<div class="my-1 border ${errCls} rounded p-2">
                <div class="flex items-center gap-1 mb-1">
                    <span class="font-semibold text-xs font-mono text-gray-700">${escapeHtml(block.tool_name || '')} result</span>
                    <span class="text-gray-400 text-xs">${escapeHtml(block.call_id || '')}</span>${errLabel}
                </div>
                <pre class="text-xs font-mono overflow-auto max-h-32 whitespace-pre-wrap">${escapeHtml(block.output || '')}</pre>
            </div>`;
        }
        case 'image':
            return `<span class="text-xs text-gray-400 italic">[Image: ${escapeHtml(block.description || '')}]</span>`;
        case 'document':
            return `<span class="text-xs text-gray-400 italic">[Document: ${escapeHtml(block.description || '')}]</span>`;
        default:
            return `<pre class="text-xs">${escapeHtml(JSON.stringify(block, null, 2))}</pre>`;
    }
}

function renderChatMessageHtml(msg, index) {
    const role = msg.role || 'unknown';
    const badgeCls = ROLE_BADGE[role] || 'bg-gray-200 text-gray-700';
    const borderCls = ROLE_BORDER[role] || 'border-l-4 border-gray-300 bg-gray-50';
    const blocks = Array.isArray(msg.blocks) ? msg.blocks : [];
    const blocksHtml = blocks.map(renderContentBlockHtml).join('');
    const meta = msg.api_usage_tokens
        ? `<span class="text-xs text-gray-400 ml-1">${msg.api_usage_tokens} tokens</span>` : '';

    // System messages are long — collapse by default
    if (role === 'system') {
        return `<details class="mb-1 rounded overflow-hidden">
            <summary class="cursor-pointer flex items-center gap-2 px-3 py-2 ${borderCls} select-none" style="list-style:none">
                <span class="px-2 py-0.5 rounded text-xs font-semibold uppercase ${badgeCls}">${escapeHtml(role)}</span>
                <span class="text-xs text-gray-400">${blocks.length} block(s) — click to expand</span>
            </summary>
            <div class="px-3 py-2 ${borderCls}">${blocksHtml}</div>
        </details>`;
    }
    return `<div class="mb-1 px-3 py-2 ${borderCls} rounded-r">
        <div class="flex items-center mb-1">
            <span class="px-2 py-0.5 rounded text-xs font-semibold uppercase ${badgeCls}">${escapeHtml(role)}</span>${meta}
        </div>
        <div class="space-y-1">${blocksHtml}</div>
    </div>`;
}

// Render a LlmRequest JSON value as structured HTML
function renderLlmRequestHtml(val) {
    let req;
    try { req = typeof val === 'string' ? JSON.parse(val) : val; } catch (_) { req = null; }
    if (!req || typeof req !== 'object') {
        const raw = typeof val === 'string' ? val : JSON.stringify(val, null, 2);
        return `<pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(raw)}</pre>`;
    }

    const messages = Array.isArray(req.messages) ? req.messages : [];
    const tools = Array.isArray(req.tools) ? req.tools : [];

    // Config bar: only show non-default values
    const configItems = [];
    if (req.tool_choice && req.tool_choice !== 'auto') configItems.push(`tool_choice: ${JSON.stringify(req.tool_choice)}`);
    if (req.max_tokens != null) configItems.push(`max_tokens: ${req.max_tokens}`);
    if (req.temperature != null) configItems.push(`temperature: ${req.temperature}`);
    const rf = req.response_format;
    if (rf && rf !== 'text') {
        const rfStr = typeof rf === 'object' ? Object.keys(rf)[0] : String(rf);
        configItems.push(`format: ${rfStr}`);
    }
    const configHtml = configItems.length > 0
        ? `<div class="flex flex-wrap gap-3 mb-2 px-3 py-1 bg-gray-100 rounded text-xs text-gray-600">${configItems.map(c => `<span>${escapeHtml(c)}</span>`).join('')}</div>`
        : '';

    // Tools list (collapsible)
    let toolsHtml = '';
    if (tools.length > 0) {
        const toolItems = tools.map(t => `<details class="ml-2">
            <summary class="cursor-pointer text-xs font-mono font-semibold text-green-700 hover:underline" style="list-style:none">${escapeHtml(t.name || '')}</summary>
            <p class="text-xs text-gray-500 ml-2 mt-0.5 mb-1">${escapeHtml(t.description || '')}</p>
        </details>`).join('');
        toolsHtml = `<details class="mb-2">
            <summary class="cursor-pointer text-xs font-semibold text-gray-500 bg-gray-100 px-3 py-1 rounded" style="list-style:none">${tools.length} tool(s) available</summary>
            <div class="mt-1 border-l-2 border-gray-200 pl-2 py-1">${toolItems}</div>
        </details>`;
    }

    const messagesHtml = messages.map((m, i) => renderChatMessageHtml(m, i)).join('');
    return `${configHtml}${toolsHtml}<div>${messagesHtml}</div>`;
}

// Render an AssistantMessage JSON value as structured HTML
function renderLlmResponseHtml(val) {
    let resp;
    try { resp = typeof val === 'string' ? JSON.parse(val) : val; } catch (_) { resp = null; }
    if (!resp || typeof resp !== 'object') {
        const raw = typeof val === 'string' ? val : JSON.stringify(val, null, 2);
        return `<pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(raw)}</pre>`;
    }

    const usage = resp.usage || {};
    const stopReason = resp.stop_reason || '';
    const stopCls = stopReason === 'end_turn' ? 'bg-green-100 text-green-700'
        : stopReason === 'tool_use' ? 'bg-blue-100 text-blue-700'
        : stopReason === 'max_tokens' ? 'bg-yellow-100 text-yellow-700'
        : 'bg-gray-100 text-gray-600';

    const parts = [];

    // Usage + stop reason bar
    const stats = [];
    if (stopReason) stats.push(`<span class="px-2 py-0.5 rounded text-xs font-semibold ${stopCls}">${escapeHtml(stopReason)}</span>`);
    if (usage.prompt_tokens != null) stats.push(`<span class="text-xs text-gray-500">↑ ${usage.prompt_tokens} prompt</span>`);
    if (usage.completion_tokens != null) stats.push(`<span class="text-xs text-gray-500">↓ ${usage.completion_tokens} completion</span>`);
    if (usage.total_tokens != null) stats.push(`<span class="text-xs text-gray-600 font-semibold">= ${usage.total_tokens} total</span>`);
    if (stats.length) parts.push(`<div class="flex flex-wrap items-center gap-2 mb-2">${stats.join('')}</div>`);

    // Text response
    if (resp.text) {
        parts.push(`<pre class="whitespace-pre-wrap text-sm font-mono break-words">${escapeHtml(resp.text)}</pre>`);
    }

    // Tool calls
    const toolCalls = Array.isArray(resp.tool_calls) ? resp.tool_calls : [];
    if (toolCalls.length > 0) {
        const callsHtml = toolCalls.map(tc => `<div class="border border-green-200 rounded p-2 mb-1 bg-white">
            <div class="flex items-center gap-2 mb-1">
                <span class="font-semibold text-green-700 text-xs font-mono">${escapeHtml(tc.tool_name || '')}</span>
                <span class="text-xs text-gray-400">${escapeHtml(tc.call_id || '')}</span>
            </div>
            <pre class="text-xs font-mono bg-gray-50 p-1 rounded overflow-auto max-h-40">${escapeHtml(JSON.stringify(tc.input, null, 2))}</pre>
        </div>`).join('');
        parts.push(`<div class="mt-2"><div class="text-xs font-semibold text-gray-600 mb-1">${toolCalls.length} tool call(s):</div>${callsHtml}</div>`);
    }

    return parts.join('');
}

// Render a key-value grid for structured span types (TURN, COMPRESSION, etc.)
// items: [{label, value, badge?, wide?}]  — items with null/undefined value are skipped
function renderKvGridHtml(items) {
    const cells = items.filter(item => item.value !== undefined && item.value !== null && item.value !== '').map(item => {
        const valueHtml = item.badge
            ? `<span class="inline-block px-2 py-0.5 rounded text-xs font-semibold ${item.badge}">${escapeHtml(String(item.value))}</span>`
            : `<span class="text-sm font-mono break-all">${escapeHtml(String(item.value))}</span>`;
        return `<div class="${item.wide ? 'col-span-2' : ''}">
            <div class="text-xs text-gray-500 mb-0.5">${escapeHtml(item.label)}</div>
            ${valueHtml}
        </div>`;
    }).join('');
    return cells
        ? `<div class="grid grid-cols-2 gap-x-6 gap-y-3 bg-gray-50 rounded p-3">${cells}</div>`
        : '<p class="text-xs text-gray-400">No fields recorded.</p>';
}

function outcomeBadge(outcome) {
    if (!outcome) return null;
    return outcome === 'Ok' ? 'bg-green-100 text-green-700'
        : outcome === 'Error' ? 'bg-red-100 text-red-700'
        : outcome === 'Cancelled' ? 'bg-yellow-100 text-yellow-700'
        : outcome === 'Denied' ? 'bg-orange-100 text-orange-700'
        : 'bg-gray-100 text-gray-600';
}

function renderTimeline(spans, animate = false) {
    try {
        if (!spans || spans.length === 0) {
            document.getElementById('timeline-content').innerHTML = '<p class="text-gray-500">No spans to display</p>';
            return;
        }

        const minTime = Math.min(...spans.map(s => s.start_time));
        const maxTime = Math.max(...spans.map(s => s.end_time || s.start_time));
        const totalDuration = Math.max(maxTime - minTime, 1);
        const now = Date.now();

        const parentMap = {};
        const extraParentMap = {};

        spans.forEach(span => {
            if (span.parent_span_id) {
                parentMap[span.span_id] = span.parent_span_id;
            }

            if (span.span_type === 'SPAWNED' && span.extras) {
                if (span.extras.parent_span_id) {
                    if (!extraParentMap[span.span_id]) {
                        extraParentMap[span.span_id] = [];
                    }
                    extraParentMap[span.span_id].push(span.extras.parent_span_id);
                }
            }

            if (span.extras && span.extras.parent_span_ids && Array.isArray(span.extras.parent_span_ids)) {
                if (!extraParentMap[span.span_id]) {
                    extraParentMap[span.span_id] = [];
                }
                span.extras.parent_span_ids.forEach(pid => {
                    if (!extraParentMap[span.span_id].includes(pid)) {
                        extraParentMap[span.span_id].push(pid);
                    }
                });
            }
        });

        const barHeight = 24;
        const barGap = 4;
        const totalHeight = spans.length * (barHeight + barGap) + 10;
        const canvasWidth = 800;
        const padding = 20;
        const minBarWidth = 60;
        const drawableWidth = canvasWidth - padding * 2 - minBarWidth;

        const sortedSpans = [...spans].sort((a, b) => a.start_time - b.start_time);

        const prevSpanIds = timelineAnimationState.prevSpans.map(s => s.span_id);

        const spanData = sortedSpans.map((span, idx) => {
            const startX = ((span.start_time - minTime) / totalDuration) * drawableWidth + padding;
            
            // USER/QUEST are root spans - use maxTime if no end_time
            // TOOL/THINK can be truly ongoing
            const isRootSpan = span.span_type === 'USER' || span.span_type === 'QUEST';
            const effectiveEndTime = span.end_time || (isRootSpan ? maxTime : null);
            const isOngoing = !effectiveEndTime && !isRootSpan;
            
            const duration = effectiveEndTime ? effectiveEndTime - span.start_time : 0;
            const barWidth = effectiveEndTime 
                ? Math.max((duration / totalDuration) * drawableWidth, minBarWidth) 
                : Math.max(((now - span.start_time) / totalDuration) * drawableWidth, minBarWidth);
            const y = idx * (barHeight + barGap) + 5;
            const isNew = animate && !prevSpanIds.includes(span.span_id);
            
            return {
                span,
                x: startX,
                y: y,
                width: barWidth,
                height: barHeight,
                centerX: startX + barWidth / 2,
                centerY: y + barHeight / 2,
                bottom: y + barHeight,
                top: y,
                isOngoing,
                isNew,
                targetX: startX,
                prevX: startX
            };
        });

        timelineAnimationState.prevSpans = sortedSpans;

        const spanByIdx = {};
        sortedSpans.forEach((span, idx) => {
            spanByIdx[span.span_id] = idx;
        });

        let hoveredIdx = -1;
        let isAutoRefreshing = false;

        document.getElementById('timeline-content').innerHTML = `
            <div class="bg-white rounded border" style="width: 100%;">
                <canvas id="timeline-canvas" style="display: block; width: 100%;"></canvas>
            </div>
        `;

        const canvas = document.getElementById('timeline-canvas');
        const container = canvas.parentElement;
        const containerWidth = container.offsetWidth || canvasWidth;
        const dpr = window.devicePixelRatio || 1;
        canvas.width = containerWidth * dpr;
        canvas.height = totalHeight * dpr;
        canvas.style.width = containerWidth + 'px';
        canvas.style.height = totalHeight + 'px';
        
        const actualPadding = 20;
        const actualDrawableWidth = containerWidth - actualPadding * 2 - minBarWidth;
        spanData.forEach((data) => {
            const span = data.span;
            const startX = ((span.start_time - minTime) / totalDuration) * actualDrawableWidth + actualPadding;
            
            const isRootSpan = span.span_type === 'USER' || span.span_type === 'QUEST';
            const effectiveEndTime = span.end_time || (isRootSpan ? maxTime : null);
            const duration = effectiveEndTime ? effectiveEndTime - span.start_time : 0;
            const barWidth = effectiveEndTime 
                ? Math.max((duration / totalDuration) * actualDrawableWidth, minBarWidth) 
                : Math.max(((now - span.start_time) / totalDuration) * actualDrawableWidth, minBarWidth);
            
            data.x = startX;
            data.targetX = startX;
            data.width = barWidth;
            data.centerX = startX + barWidth / 2;
        });

        const ctx = canvas.getContext('2d');
        ctx.scale(dpr, dpr);

        function drawOngoingIndicator(x, y) {
            const time = Date.now() / 1000;
            const angle = time * Math.PI * 2;

            ctx.save();
            ctx.translate(x, y);
            ctx.rotate(angle);

            ctx.beginPath();
            ctx.arc(0, 0, 6, 0, Math.PI * 1.5);
            ctx.strokeStyle = '#22c55e';
            ctx.lineWidth = 2;
            ctx.stroke();

            ctx.beginPath();
            ctx.moveTo(6, 0);
            ctx.lineTo(6, -3);
            ctx.lineTo(3, 0);
            ctx.closePath();
            ctx.fillStyle = '#22c55e';
            ctx.fill();

            ctx.restore();
        }

        function draw(animProgress = 1) {
            ctx.clearRect(0, 0, containerWidth, totalHeight);

            const progress = easeOutCubic(animProgress);

            spanData.forEach(data => {
                let parents = [];
                if (extraParentMap[data.span.span_id] && extraParentMap[data.span.span_id].length > 0) {
                    parents = extraParentMap[data.span.span_id];
                } else if (data.span.parent_span_id) {
                    parents = [data.span.parent_span_id];
                }

                parents.forEach(parentId => {
                    const parentIdx = spanByIdx[parentId];
                    if (parentIdx === undefined) return;
                    const parentData = spanData[parentIdx];

                    const fromX = parentData.x + parentData.width - 4;
                    const fromY = parentData.bottom;
                    const toX = data.x;
                    const toY = data.centerY;

                    const cp1x = fromX + 20;
                    const cp1y = fromY + 10;
                    const cp2x = toX - 20;
                    const cp2y = toY - 10;

                    ctx.beginPath();
                    ctx.lineWidth = 1.5;
                    ctx.strokeStyle = '#9ca3af';
                    ctx.moveTo(fromX, fromY);
                    ctx.bezierCurveTo(cp1x, cp1y, cp2x, cp2y, toX, toY);
                    ctx.stroke();
                });
            });

            spanData.forEach((data, idx) => {
                const colors = SPAN_COLORS[data.span.span_type] || SPAN_COLORS[getCompatibleSpanType(data.span.span_type)] || SPAN_COLORS['END'];
                const isHovered = idx === hoveredIdx;

                let x = data.x;
                let opacity = 1;

                if (data.isNew && animProgress < 1) {
                    const bounceProgress = easeOutBack(progress);
                    x = data.targetX + 50 * (1 - bounceProgress);
                    opacity = progress;
                }

                ctx.globalAlpha = opacity;

                ctx.beginPath();
                ctx.fillStyle = isHovered ? colors.hover : colors.bg;
                ctx.roundRect(x, data.y, data.width, data.height, 4);
                ctx.fill();

                ctx.beginPath();
                ctx.strokeStyle = colors.border;
                ctx.lineWidth = isHovered ? 2 : 1;
                ctx.roundRect(x, data.y, data.width, data.height, 4);
                ctx.stroke();

                const duration = data.span.end_time 
                    ? (data.span.end_time - data.span.start_time) + 'ms' 
                    : 'ongoing';
                const text = `${data.span.span_type} ${duration}`;
                ctx.fillStyle = colors.text;
                ctx.font = isHovered ? 'bold 11px system-ui, sans-serif' : '11px system-ui, sans-serif';
                ctx.textBaseline = 'middle';
                ctx.fillText(text, x + 6, data.y + data.height / 2);

                if (data.isOngoing) {
                    drawOngoingIndicator(x + data.width - 12, data.y + data.height / 2);
                }

                ctx.globalAlpha = 1;
            });
        }

        function animateTimeline() {
            const elapsed = Date.now() - timelineAnimationState.animationStartTime;
            const duration = 400;
            timelineAnimationState.animationProgress = Math.min(elapsed / duration, 1);

            draw(timelineAnimationState.animationProgress);

            if (timelineAnimationState.animationProgress < 1) {
                requestAnimationFrame(animateTimeline);
            }
        }

        function updateOngoingSpans() {
            let needsRedraw = false;
            const nowInner = Date.now();

            spanData.forEach(data => {
                if (data.isOngoing) {
                    const newWidth = Math.max(((nowInner - data.span.start_time) / totalDuration) * actualDrawableWidth, minBarWidth);
                    if (Math.abs(newWidth - data.width) > 1) {
                        data.width = newWidth;
                        needsRedraw = true;
                    }
                }
            });

            if (needsRedraw) {
                draw(timelineAnimationState.animationProgress);
            }

            if (isAutoRefreshing) {
                timelineAnimationState.ongoingAnimationId = requestAnimationFrame(updateOngoingSpans);
            }
        }

        if (animate) {
            timelineAnimationState.animationProgress = 0;
            timelineAnimationState.animationStartTime = Date.now();
            animateTimeline();
        } else {
            draw(1);
        }

        function findSpanAtPos(x, y) {
            for (let i = spanData.length - 1; i >= 0; i--) {
                const data = spanData[i];
                if (x >= data.x && x <= data.x + data.width && 
                    y >= data.y && y <= data.y + data.height) {
                    return i;
                }
            }
            return -1;
        }

        canvas.addEventListener('mousemove', (e) => {
            const rect = canvas.getBoundingClientRect();
            const x = (e.clientX - rect.left) * (containerWidth / rect.width);
            const y = (e.clientY - rect.top) * (totalHeight / rect.height);

            const newHovered = findSpanAtPos(x, y);
            if (newHovered !== hoveredIdx) {
                hoveredIdx = newHovered;
                canvas.style.cursor = hoveredIdx >= 0 ? 'pointer' : 'default';
                draw(timelineAnimationState.animationProgress);
            }
        });

        canvas.addEventListener('mouseleave', () => {
            if (hoveredIdx !== -1) {
                hoveredIdx = -1;
                draw(timelineAnimationState.animationProgress);
            }
        });

        canvas.addEventListener('click', (e) => {
            const rect = canvas.getBoundingClientRect();
            const x = (e.clientX - rect.left) * (containerWidth / rect.width);
            const y = (e.clientY - rect.top) * (totalHeight / rect.height);

            const idx = findSpanAtPos(x, y);
            if (idx >= 0) {
                showSpanDetailsInline(spanData[idx].span.span_id);
            }
        });

        window.timelineSetAutoRefresh = (auto) => {
            isAutoRefreshing = auto;
            if (auto) {
                updateOngoingSpans();
            } else if (timelineAnimationState.ongoingAnimationId) {
                cancelAnimationFrame(timelineAnimationState.ongoingAnimationId);
            }
        };

    } catch (error) {
        document.getElementById('timeline-content').innerHTML =
            `<p class="text-red-600">Error rendering timeline: ${error.message}</p>`;
    }
}

async function showSpanDetailsInline(spanId) {
    try {
        const response = await fetch(`/api/spans/${spanId}`);
        const span = await response.json();
        currentSpan = span;
        const detailCompat = getSpanDetailCompatibility(span);
        
        let inputOutputHtml = '';
        if (detailCompat.compatibleType === 'THINK' && detailCompat.extras) {
            if (detailCompat.llmInput != null) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Request:</label>
                        <div class="bg-blue-50 border border-blue-200 rounded-lg overflow-auto p-4" style="max-height: 80vh;">
                            ${renderLlmRequestHtml(detailCompat.llmInput)}
                        </div>
                    </div>
                `;
            }
            if (detailCompat.llmOutput != null) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Response:</label>
                        <div class="bg-green-50 border border-green-200 rounded-lg overflow-auto p-4" style="max-height: 80vh;">
                            ${renderLlmResponseHtml(detailCompat.llmOutput)}
                        </div>
                    </div>
                `;
            }
        }
        
        if (detailCompat.compatibleType === 'TOOL' && detailCompat.extras) {
            const toolName = formatExtraValue(detailCompat.toolName);
            const toolInput = formatExtraValue(detailCompat.toolInput);
            const toolOutput = formatExtraValue(detailCompat.toolOutput);
            const toolStdout = formatExtraValue(detailCompat.toolStdout);
            const toolStderr = formatExtraValue(detailCompat.toolStderr);
            const toolStatus = detailCompat.toolStatus;
            const toolResultClass = toolStatus && toolStatus.success === true ? 'bg-green-50 border-green-200' :
                toolStatus && toolStatus.success === false ? 'bg-red-50 border-red-200' : 'bg-gray-50 border-gray-200';

            if (toolName !== null) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Tool Name:</label>
                        <p class="font-mono text-sm bg-gray-100 px-3 py-2 rounded">${escapeHtml(toolName)}</p>
                    </div>
                `;
            }
            if (toolInput !== null) {
                inputOutputHtml += createCollapsiblePanel(
                    'tool-input-' + span.span_id,
                    'Tool Input',
                    toolInput,
                    'bg-blue-50',
                    'border-blue-200'
                );
            }
            if (toolOutput !== null) {
                inputOutputHtml += createCollapsiblePanel(
                    'tool-output-' + span.span_id,
                    'Tool Output',
                    toolOutput,
                    toolResultClass.split(' ')[0] || 'bg-gray-50',
                    toolResultClass.split(' ')[1] || 'border-gray-200'
                );
            }
            if (toolStdout !== null && toolStdout !== '') {
                inputOutputHtml += createCollapsiblePanel(
                    'stdout-' + span.span_id,
                    'Stdout',
                    toolStdout,
                    toolResultClass.split(' ')[0] || 'bg-gray-50',
                    toolResultClass.split(' ')[1] || 'border-gray-200'
                );
            }
            if (toolStderr !== null && toolStderr !== '') {
                inputOutputHtml += createCollapsiblePanel(
                    'stderr-' + span.span_id,
                    'Stderr',
                    toolStderr,
                    'bg-red-50',
                    'border-red-200',
                    'text-red-800'
                );
            }
            if (toolStatus) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Status:</label>
                        <span class="px-3 py-1 rounded text-sm font-semibold ${toolStatus.className}">${escapeHtml(toolStatus.text)}</span>
                    </div>
                `;
            }
        }
        
        if (detailCompat.compatibleType === 'TURN' && detailCompat.extras) {
            const e = detailCompat.extras;
            inputOutputHtml += `<div class="mb-4">${renderKvGridHtml([
                { label: 'Turn #', value: e.turn_number },
                { label: 'Agent', value: e.agent_id },
                { label: 'Outcome', value: e.outcome, badge: outcomeBadge(e.outcome) },
                { label: 'Stop Reason', value: e.stop_reason },
                { label: '↑ Prompt Tokens', value: e.prompt_tokens },
                { label: '↓ Completion Tokens', value: e.completion_tokens },
                { label: '= Total Tokens', value: e.total_tokens },
                { label: 'Has Tool Calls', value: e.has_tool_calls != null ? String(e.has_tool_calls) : undefined },
            ])}</div>`;
        }

        if (detailCompat.compatibleType === 'COMPRESSION' && detailCompat.extras) {
            const e = detailCompat.extras;
            const usageRatio = e.usage_ratio != null ? `${(e.usage_ratio * 100).toFixed(1)}%` : undefined;
            const skippedBadge = e.skipped === true ? 'bg-gray-100 text-gray-600'
                : e.skipped === false ? 'bg-orange-100 text-orange-700' : null;
            const skippedLabel = e.skipped === true ? 'skipped' : e.skipped === false ? 'compressed' : undefined;
            inputOutputHtml += `<div class="mb-4">${renderKvGridHtml([
                { label: 'Status', value: skippedLabel, badge: skippedBadge },
                { label: 'Outcome', value: e.outcome, badge: outcomeBadge(e.outcome) },
                { label: 'Severity', value: e.severity },
                { label: 'Usage Ratio', value: usageRatio },
                { label: 'Estimated Tokens', value: e.estimated_tokens },
                { label: 'Available Tokens', value: e.available_tokens },
                { label: 'Messages (before)', value: e.messages_before },
                { label: 'Messages (after)', value: e.messages_after },
                { label: 'Removed', value: e.removed_count },
                { label: 'Has Summary', value: e.has_summary != null ? String(e.has_summary) : undefined },
                { label: 'Tokens After', value: e.estimated_tokens_after },
                { label: 'Error', value: e.error, wide: true },
            ])}</div>`;
        }

        if (detailCompat.compatibleType === 'PROMPT_BUILD' && detailCompat.extras) {
            const e = detailCompat.extras;
            inputOutputHtml += `<div class="mb-4">${renderKvGridHtml([
                { label: 'Turn #', value: e.turn_number },
                { label: 'Agent', value: e.agent_id },
                { label: 'Outcome', value: e.outcome, badge: outcomeBadge(e.outcome) },
                { label: 'Messages', value: e.message_count },
                { label: 'Visible Tools', value: e.visible_tool_count },
                { label: 'Skills', value: e.skill_count },
                { label: 'Has System Prompt', value: e.has_system_prompt != null ? String(e.has_system_prompt) : undefined },
                { label: 'Est. Input Tokens', value: e.estimated_input_tokens },
                { label: 'Request Messages', value: e.request_message_count },
                { label: 'Error', value: e.error, wide: true },
            ])}</div>`;
        }

        if (detailCompat.compatibleType === 'HOOK' && detailCompat.extras) {
            const e = detailCompat.extras;
            const resultGood = new Set(['allow', 'accept', 'transform', 'recover']);
            const resultBad  = new Set(['deny', 'propagate']);
            const resultBadge = e.result
                ? (resultGood.has(e.result) ? 'bg-green-100 text-green-700'
                   : resultBad.has(e.result) ? 'bg-red-100 text-red-700'
                   : 'bg-gray-100 text-gray-600')
                : null;
            inputOutputHtml += `<div class="mb-4">${renderKvGridHtml([
                { label: 'Hook Kind', value: e.hook_kind },
                { label: 'Result', value: e.result, badge: resultBadge },
                { label: 'Outcome', value: e.outcome, badge: outcomeBadge(e.outcome) },
                { label: 'Hooker ID', value: e.hooker_id, wide: true },
                { label: 'Hook Point', value: e.hook_point, wide: true },
                { label: 'Tool Name', value: e.tool_name },
                { label: 'Call ID', value: e.call_id },
                { label: 'Error', value: e.error, wide: true },
            ])}</div>`;
        }

        let extrasDisplay = '';
        if (span.extras && Object.keys(span.extras).length > 0) {
            let filteredExtras = { ...span.extras };
            for (const key of detailCompat.promotedKeys) {
                delete filteredExtras[key];
            }
            if (Object.keys(filteredExtras).length > 0) {
                extrasDisplay = `
                    <div class="mb-4">
                        <label class="font-semibold">Other Info:</label>
                        <pre class="bg-gray-100 p-4 rounded mt-2 overflow-auto max-h-screen">${JSON.stringify(filteredExtras, null, 2)}</pre>
                    </div>
                `;
            }
        }
        
        const html = `
            <div>
                <div class="grid grid-cols-2 gap-4 mb-4">
                    <div>
                        <label class="font-semibold text-gray-700">Span ID:</label>
                        <p class="font-mono text-sm">${span.span_id}</p>
                    </div>
                    <div>
                        <label class="font-semibold text-gray-700">Type:</label>
                        <p><span class="px-2 py-0.5 rounded text-xs font-semibold ${getSpanTypeColor(getCompatibleSpanType(span.span_type))}">${span.span_type}</span></p>
                    </div>
                    <div>
                        <label class="font-semibold text-gray-700">Start Time:</label>
                        <p class="text-sm">${formatTime(span.start_time)}</p>
                    </div>
                    <div>
                        <label class="font-semibold text-gray-700">End Time:</label>
                        <p class="text-sm">${span.end_time ? formatTime(span.end_time) : 'ongoing'}</p>
                    </div>
                </div>
                ${inputOutputHtml}
                ${extrasDisplay}
                <button onclick="navigator.clipboard.writeText('${span.span_id}')" 
                        class="px-3 py-1 bg-blue-600 text-white text-sm rounded hover:bg-blue-700">
                    Copy Span ID
                </button>
            </div>
        `;
        
        document.getElementById('detail-content').innerHTML = html;
    } catch (error) {
        document.getElementById('detail-content').innerHTML = 
            `<p class="text-red-600">Error loading span details: ${error.message}</p>`;
    }
}

function showSpanDetails(spanId) {
    showSpanDetailsInline(spanId);
}
