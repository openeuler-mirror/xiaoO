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
    'END': { bg: '#e5e7eb', border: '#6b7280', hover: '#d1d5db', text: '#374151' }
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

const DEDICATED_DETAIL_KEYS = {
    'THINK': ['input_preview', 'effective_request', 'output_preview', 'final_response'],
    'TOOL': ['tool_name', 'input', 'effective_input', 'output', 'final_output', 'stdout', 'stderr', 'success', 'result_kind']
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
    const promotedKeys = new Set(DEDICATED_DETAIL_KEYS[compatibleType] || []);

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
            const llmInput = formatExtraValue(detailCompat.llmInput);
            const llmOutput = formatExtraValue(detailCompat.llmOutput);

            if (llmInput !== null) {
                inputOutputHtml += createCollapsiblePanel(
                    'llm-input-' + span.span_id,
                    'Input',
                    llmInput,
                    'bg-blue-50',
                    'border-blue-200'
                );
            }
            if (llmOutput !== null) {
                inputOutputHtml += createCollapsiblePanel(
                    'llm-output-' + span.span_id,
                    'Output',
                    llmOutput,
                    'bg-green-50',
                    'border-green-200'
                );
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
