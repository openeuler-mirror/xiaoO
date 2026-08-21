// xiaoo-daemon dashboard front-end. Polls the read-only JSON endpoints
// every 5 seconds and re-renders the tables. No external dependencies.

const REFRESH_INTERVAL_MS = 5000;
const BACKEND_LINK_ATTR = "data-backend-id";

let sessions = [];
let sandboxes = [];

function setStatus(state, text) {
  const dot = document.getElementById("status-dot");
  const label = document.getElementById("status-text");
  dot.dataset.state = state;
  label.textContent = text;
}

async function fetchJson(url) {
  const resp = await fetch(url, { cache: "no-store" });
  if (!resp.ok) {
    throw new Error(`${resp.status} ${resp.statusText}`);
  }
  return resp.json();
}

function shortId(id) {
  if (!id) return "";
  return id.length <= 16 ? id : id.slice(0, 8) + "…" + id.slice(-4);
}

function relativeTime(ms) {
  if (!ms || ms <= 0) return "—";
  const now = Date.now();
  const diff = Math.max(0, now - ms);
  if (diff < 60_000) return Math.floor(diff / 1000) + "s ago";
  if (diff < 3_600_000) return Math.floor(diff / 60_000) + "m ago";
  if (diff < 86_400_000) return Math.floor(diff / 3_600_000) + "h ago";
  return Math.floor(diff / 86_400_000) + "d ago";
}

function formatTime(ms) {
  if (!ms || ms <= 0) return "—";
  return new Date(ms).toISOString().replace("T", " ").slice(0, 19) + "Z";
}

// Render a BackendEndpoint (Option<BackendEndpoint> on the Rust side) as a
// readable string. serde serializes the unit variant `Local` as the bare
// string "local" and the struct variants as internally-tagged objects
// ({ "tcp": {...} }, { "unix_socket": {...} }, { "provider_handle": {...} }),
// so a raw String() would yield "[object Object]" for the latter. Output
// mirrors the Rust `backend_endpoint_str` so the sandbox table matches the
// session table's `backend_endpoint` column.
function endpointLabel(endpoint) {
  if (endpoint == null) return "—";
  if (typeof endpoint === "string") return endpoint;
  if (endpoint.tcp) {
    return `tcp://${endpoint.tcp.host}:${endpoint.tcp.port}`;
  }
  if (endpoint.unix_socket) {
    return "unix:" + endpoint.unix_socket.path;
  }
  if (endpoint.provider_handle) {
    return "provider:" + JSON.stringify(endpoint.provider_handle.value);
  }
  return JSON.stringify(endpoint);
}

function el(tag, attrs = {}, children = []) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") node.className = v;
    else if (k === "text") node.textContent = v;
    else if (k === "html") node.innerHTML = v;
    else if (k.startsWith("on") && typeof v === "function") {
      node.addEventListener(k.slice(2), v);
    } else if (v !== null && v !== undefined) {
      node.setAttribute(k, v);
    }
  }
  for (const c of [].concat(children)) {
    if (c == null) continue;
    node.appendChild(typeof c === "string" ? document.createTextNode(c) : c);
  }
  return node;
}

function renderOverview(o) {
  document.getElementById("sessions-total").textContent = o.sessions.total;
  document.getElementById("sandboxes-total").textContent = o.sandboxes.total;
  document.getElementById("sessions-no-sandbox").textContent =
    o.sessions_without_sandbox ?? 0;
  document.getElementById("orphan-sandboxes").textContent =
    o.orphan_sandboxes ?? 0;
  document.getElementById("server-time").textContent = formatTime(
    o.server_time_ms
  );

  const sb = document.getElementById("sessions-breakdown");
  sb.innerHTML = "";
  for (const [k, v] of Object.entries(o.sessions.by_status || {})) {
    sb.appendChild(el("li", {}, [el("span", { class: "k", text: k }), el("span", { class: "v", text: String(v) })]));
  }
  const pb = document.getElementById("sandboxes-breakdown");
  pb.innerHTML = "";
  for (const [k, v] of Object.entries(o.sandboxes.by_provider || {})) {
    pb.appendChild(el("li", {}, [el("span", { class: "k", text: k }), el("span", { class: "v", text: String(v) })]));
  }
}

function renderSessions(rows) {
  const tbody = document.getElementById("sessions-tbody");
  tbody.innerHTML = "";
  document.getElementById("sessions-count").textContent = String(rows.length);

  if (rows.length === 0) {
    tbody.appendChild(
      el("tr", {}, [el("td", { class: "empty", colspan: 9, text: "no sessions" })])
    );
    return;
  }

  for (const s of rows) {
    const statusBadge = el("span", {
      class: "badge",
      "data-status": s.status,
      text: s.status,
    });
    const backendCell = s.backend_id
      ? el("a", {
          class: "backend-link mono",
          href: "#sandbox-" + s.backend_id,
          [BACKEND_LINK_ATTR]: s.backend_id,
          text: shortId(s.backend_id),
          title: s.backend_id,
        })
      : el("span", { class: "muted", text: "—" });
    const backendState = s.backend_state
      ? el("span", {
          class: "badge",
          "data-state": s.backend_state,
          text: s.backend_state,
        })
      : el("span", { class: "muted", text: "—" });

    tbody.appendChild(
      el("tr", {}, [
        el("td", { class: "mono", text: s.session_id, title: s.session_id }),
        el("td", {}, [statusBadge]),
        el("td", {}, [el("span", { class: "mono", text: s.agent_id || "—" })]),
        el("td", {}, [el("span", { class: "mono", text: s.model || "—" })]),
        el("td", { text: s.channel || "—" }),
        el("td", { class: "mono", text: shortId(s.conversation_id), title: s.conversation_id || "" }),
        el("td", {}, [backendCell]),
        el("td", {}, [backendState]),
        el("td", { text: relativeTime(s.updated_at_ms), title: formatTime(s.updated_at_ms) }),
      ])
    );
  }
}

function renderSandboxes(rows) {
  const tbody = document.getElementById("sandboxes-tbody");
  tbody.innerHTML = "";
  document.getElementById("sandboxes-count").textContent = String(rows.length);

  if (rows.length === 0) {
    tbody.appendChild(
      el("tr", {}, [el("td", { class: "empty", colspan: 7, text: "no sandboxes" })])
    );
    return;
  }

  for (const b of rows) {
    const state = String(b.state || "unknown");
    const endpoint = endpointLabel(b.endpoint);
    tbody.appendChild(
      el("tr", { id: "sandbox-" + b.backend_id }, [
        el("td", { class: "mono", text: b.backend_id, title: b.backend_id }),
        el("td", { text: b.provider || "—" }),
        el("td", {}, [el("span", { class: "badge", "data-state": state, text: state })]),
        el("td", { class: "mono", text: b.workspace_root || "—", title: b.workspace_root || "" }),
        el("td", { class: "mono", text: endpoint, title: endpoint }),
        el("td", { class: "mono", text: (b.session_ids || []).join(", ") || "—" }),
        el("td", { text: b.expires_at_ms ? formatTime(b.expires_at_ms) : "—" }),
      ])
    );
  }
}

async function refresh() {
  try {
    const [overview, sessionsData, sandboxesData] = await Promise.all([
      fetchJson("/api/v1/dashboard/overview"),
      fetchJson("/api/v1/dashboard/sessions"),
      fetchJson("/api/v1/dashboard/sandboxes"),
    ]);
    sessions = sessionsData;
    sandboxes = sandboxesData;

    renderOverview(overview);
    renderSessions(sessions);
    renderSandboxes(sandboxes);
    setStatus("ok", "live");
  } catch (err) {
    setStatus("err", "error: " + (err && err.message ? err.message : String(err)));
  }
}

window.addEventListener("DOMContentLoaded", () => {
  refresh();
  setInterval(refresh, REFRESH_INTERVAL_MS);
});
