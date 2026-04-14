const SPAN_COLORS = {
    'USER': { bg: '#bfdbfe', border: '#3b82f6', hover: '#93c5fd', text: '#1e40af' },
    'QUEST': { bg: '#e9d5ff', border: '#a855f7', hover: '#d8b4fe', text: '#6b21a8' },
    'SPAWNED': { bg: '#cffafe', border: '#06b6d4', hover: '#a5f3fc', text: '#0e7490' },
    'THINK': { bg: '#c7d2fe', border: '#6366f1', hover: '#a5b4fc', text: '#4338ca' },
    'TOOL': { bg: '#bbf7d0', border: '#22c55e', hover: '#86efac', text: '#166534' },
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
                const colors = SPAN_COLORS[data.span.span_type] || SPAN_COLORS['END'];
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
        
        let inputOutputHtml = '';
        if (span.span_type === 'THINK' && span.extras) {
            const extras = span.extras;
            if (extras.input_preview !== undefined) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Input:</label>
                        <div class="bg-blue-50 border border-blue-200 rounded-lg overflow-hidden">
                            <div class="overflow-auto p-4 border-b-4 border-blue-100 hover:border-blue-300 transition-colors" 
                                 style="resize: vertical; min-height: 80px; max-height: 80vh;">
                                <pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(String(extras.input_preview))}</pre>
                            </div>
                        </div>
                    </div>
                `;
            }
            if (extras.output_preview !== undefined) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Output:</label>
                        <div class="bg-green-50 border border-green-200 rounded-lg overflow-hidden">
                            <div class="overflow-auto p-4 border-b-4 border-green-100 hover:border-green-300 transition-colors" 
                                 style="resize: vertical; min-height: 80px; max-height: 80vh;">
                                <pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(String(extras.output_preview))}</pre>
                            </div>
                        </div>
                    </div>
                `;
            }
        }
        
        if (span.span_type === 'TOOL' && span.extras) {
            const extras = span.extras;
            if (extras.tool_name !== undefined) {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Tool Name:</label>
                        <p class="font-mono text-sm bg-gray-100 px-3 py-2 rounded">${escapeHtml(String(extras.tool_name))}</p>
                    </div>
                `;
            }
            if (extras.input !== undefined && extras.input !== null) {
                const inputStr = typeof extras.input === 'object' ? JSON.stringify(extras.input, null, 2) : String(extras.input);
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Tool Input:</label>
                        <div class="bg-blue-50 border border-blue-200 rounded-lg overflow-hidden">
                            <div class="overflow-auto p-4 border-b-4 border-blue-100 hover:border-blue-300 transition-colors" 
                                 style="resize: vertical; min-height: 80px; max-height: 80vh;">
                                <pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(inputStr)}</pre>
                            </div>
                        </div>
                    </div>
                `;
            }
            if (extras.output !== undefined && extras.output !== null) {
                const outputStr = typeof extras.output === 'object' ? JSON.stringify(extras.output, null, 2) : String(extras.output);
                const successClass = extras.success === true ? 'bg-green-50 border-green-200' : 
                                    extras.success === false ? 'bg-red-50 border-red-200' : 'bg-gray-50 border-gray-200';
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Tool Output:</label>
                        <div class="${successClass} border rounded-lg overflow-hidden">
                            <div class="overflow-auto p-4" 
                                 style="resize: vertical; min-height: 80px; max-height: 80vh;">
                                <pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(outputStr)}</pre>
                            </div>
                        </div>
                    </div>
                `;
            }
            if (extras.stdout !== undefined && extras.stdout !== null && extras.stdout !== '') {
                const successClass = extras.success === true ? 'bg-green-50 border-green-200' : 
                                    extras.success === false ? 'bg-red-50 border-red-200' : 'bg-gray-50 border-gray-200';
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Stdout:</label>
                        <div class="${successClass} border rounded-lg overflow-hidden">
                            <div class="overflow-auto p-4" 
                                 style="resize: vertical; min-height: 80px; max-height: 80vh;">
                                <pre class="whitespace-pre-wrap text-sm font-mono">${escapeHtml(String(extras.stdout))}</pre>
                            </div>
                        </div>
                    </div>
                `;
            }
            if (extras.stderr !== undefined && extras.stderr !== null && extras.stderr !== '') {
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Stderr:</label>
                        <div class="bg-red-50 border border-red-200 rounded-lg overflow-hidden">
                            <div class="overflow-auto p-4" 
                                 style="resize: vertical; min-height: 80px; max-height: 80vh;">
                                <pre class="whitespace-pre-wrap text-sm font-mono text-red-800">${escapeHtml(String(extras.stderr))}</pre>
                            </div>
                        </div>
                    </div>
                `;
            }
            if (extras.success !== undefined) {
                const statusClass = extras.success ? 'text-green-600 bg-green-100' : 'text-red-600 bg-red-100';
                const statusText = extras.success ? '✓ Success' : '✗ Failed';
                inputOutputHtml += `
                    <div class="mb-4">
                        <label class="font-semibold text-gray-700 mb-2 block">Status:</label>
                        <span class="px-3 py-1 rounded text-sm font-semibold ${statusClass}">${statusText}</span>
                    </div>
                `;
            }
        }
        
        let extrasDisplay = '';
        if (span.extras && Object.keys(span.extras).length > 0) {
            let filteredExtras = { ...span.extras };
            if (span.span_type === 'THINK') {
                delete filteredExtras.input_preview;
                delete filteredExtras.output_preview;
            }
            if (span.span_type === 'TOOL') {
                delete filteredExtras.tool_name;
                delete filteredExtras.input;
                delete filteredExtras.output;
                delete filteredExtras.success;
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
                        <p><span class="px-2 py-0.5 rounded text-xs font-semibold ${getSpanTypeColor(span.span_type)}">${span.span_type}</span></p>
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
