"use strict";
const $ = (s, root = document) => root.querySelector(s);
const $$ = (s, root = document) => [...root.querySelectorAll(s)];
const state = { settings: null, dashboard: null, results: [], tab: "overview", timer: null };

const STATUS = {
  queued: ["排队中", "info"], checking: ["检查中", "info"], unlocking: ["解锁中", "info"], transferring: ["转存中", "info"],
  success: ["成功", "ok"], skipped: ["已跳过", "muted"], failed: ["失败", "bad"], needs_review: ["待核对", "warn"],
};

function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs || {})) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "dataset") Object.assign(el.dataset, v);
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) if (c != null && c !== false) el.append(c instanceof Node ? c : String(c));
  return el;
}

async function api(path, { method = "GET", body } = {}) {
  const opts = { method, credentials: "same-origin", headers: { Accept: "application/json" } };
  if (method !== "GET") opts.headers["X-Re0drive-Request"] = "1";
  if (body !== undefined) { opts.headers["Content-Type"] = "application/json"; opts.body = JSON.stringify(body); }
  const res = await fetch(path, opts);
  let data = null;
  try { data = await res.json(); } catch { /* empty */ }
  if (res.status === 401 && !path.startsWith("/api/auth/")) { showAuth(true); throw new Error("请先登录"); }
  if (!res.ok) throw new Error((data && data.error) || `请求失败（HTTP ${res.status}）`);
  return data;
}

let toastTimer;
function toast(msg, kind = "ok") {
  const t = $("#toast");
  t.textContent = msg; t.className = `toast ${kind}`; t.hidden = false;
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { t.hidden = true; }, 4000);
}
const fail = (e) => toast(e.message || String(e), "bad");

function fmtTime(ts) {
  if (!ts) return "—";
  const d = new Date(ts * 1000), now = Date.now() / 1000, diff = now - ts;
  if (diff >= 0 && diff < 60) return "刚刚";
  if (diff >= 0 && diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 0 && diff > -3600) return `${Math.ceil(-diff / 60)} 分钟后`;
  return d.toLocaleString("zh-CN", { hour12: false, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" });
}
const pointsLabel = (r) => r.is_unlocked ? "已拥有" : r.unlock_points == null ? "未知" : `${r.unlock_points} 分`;

function formData(form) {
  const out = {};
  for (const el of form.elements) {
    if (!el.name) continue;
    if (el.type === "checkbox") out[el.name] = el.checked;
    else if (el.type === "number") out[el.name] = el.value === "" ? null : Number(el.value);
    else out[el.name] = el.value;
  }
  return out;
}
function fillForm(form, data) {
  for (const el of form.elements) {
    if (!el.name || !(el.name in data)) continue;
    if (el.type === "checkbox") el.checked = !!data[el.name];
    else el.value = data[el.name] ?? "";
  }
}
async function busy(btn, fn) {
  if (btn) btn.disabled = true;
  try { return await fn(); } catch (e) { fail(e); } finally { if (btn) btn.disabled = false; }
}

/* ---------- auth ---------- */
let authMode = "login";
async function showAuth(force) {
  stopPolling();
  const s = await api("/api/auth/status").catch(() => ({ configured: true, authenticated: false }));
  if (s.authenticated && !force) return startApp();
  authMode = s.configured ? "login" : "setup";
  $("#auth-hint").textContent = s.configured ? "登录管理后台" : "首次使用：创建管理员账号（密码至少 8 位）";
  $("#auth-submit").textContent = s.configured ? "登录" : "创建并登录";
  $("#auth-form").password.autocomplete = s.configured ? "current-password" : "new-password";
  $("#app-view").hidden = true; $("#auth-view").hidden = false; $("#auth-error").hidden = true;
}
$("#auth-form").addEventListener("submit", async (e) => {
  e.preventDefault();
  const f = e.target;
  await busy($("#auth-submit"), async () => {
    try {
      await api(authMode === "setup" ? "/api/auth/setup" : "/api/auth/login", { method: "POST", body: { username: f.username.value, password: f.password.value } });
      f.password.value = "";
      startApp();
    } catch (err) {
      $("#auth-error").textContent = err.message === "请先登录" ? "用户名或密码错误" : err.message;
      $("#auth-error").hidden = false;
    }
  });
});
$("#logout").addEventListener("click", async () => { await api("/api/auth/logout", { method: "POST" }).catch(() => {}); showAuth(true); });

/* ---------- app shell ---------- */
async function startApp() {
  $("#auth-view").hidden = true; $("#app-view").hidden = false;
  await loadSettings().catch(fail);
  await refresh();
  startPolling();
  const hash = location.hash.slice(1);
  switchTab(hash && $(`[data-tab="${hash}"]`) ? hash : state.tab);
}
function startPolling() { stopPolling(); state.timer = setInterval(() => { if (!document.hidden) refresh(); }, 5000); }
function stopPolling() { if (state.timer) clearInterval(state.timer); state.timer = null; }

function switchTab(tab) {
  state.tab = tab;
  $$("#tabs button").forEach((b) => b.classList.toggle("active", b.dataset.tab === tab));
  $$("[data-view]").forEach((v) => { v.hidden = v.dataset.view !== tab; });
  if (location.hash.slice(1) !== tab) history.replaceState(null, "", `#${tab}`);
}
$("#tabs").addEventListener("click", (e) => { const b = e.target.closest("button[data-tab]"); if (b) switchTab(b.dataset.tab); });

async function refresh() {
  try {
    const d = await api("/api/dashboard");
    state.dashboard = d;
    applySettingsBadge(d.settings);
    renderStats(d.stats); renderActivity(d.activity); renderSubs(d.subscriptions); renderJobs(d.jobs);
  } catch (e) { if (e.message !== "请先登录") console.warn(e); }
}

function applySettingsBadge(s) {
  const demo = s.mode === "demo";
  const badge = $("#mode-badge");
  badge.textContent = demo ? "演示模式" : "中转 API";
  badge.className = `badge ${demo ? "warn" : "ok"}`;
  $("#demo-banner").hidden = !demo;
  $("#ov-checkin-day").textContent = `自动签到：${s.checkin_enabled ? "已开启" : "未开启"} · 上次签到：${s.last_checkin_day || "无记录"}`;
}

function renderStats(st) {
  $("#st-subs").textContent = st.subscriptions; $("#st-pending").textContent = st.pending;
  $("#st-success").textContent = st.success; $("#st-review").textContent = st.review;
}
function renderActivity(list) {
  const ul = $("#activity");
  ul.replaceChildren(...(list.length ? list.slice(0, 30).map((a) =>
    h("li", { class: `act-${a.kind}` }, h("span", { class: "muted small" }, fmtTime(a.created_at)), " ", a.message))
    : [h("li", { class: "muted" }, "暂无动态")]));
}

/* ---------- overview: account ---------- */
async function loadAccount(btn) {
  await busy(btn, async () => {
    const box = $("#ov-account");
    box.textContent = "查询中…";
    try {
      const s = await api("/api/accounts/re0/status");
      if (s.demo) { box.textContent = s.message || "演示模式"; return; }
      if (!s.authorized) { box.replaceChildren("尚未授权。请到「设置」获取授权链接并在浏览器中完成授权。"); return; }
      const u = s.user || {};
      const pick = (...k) => k.map((x) => u[x]).find((v) => v != null && v !== "");
      box.replaceChildren(h("div", { class: "kv" },
        h("span", {}, "用户"), h("b", {}, pick("nickname", "username", "name") ?? "已授权"),
        h("span", {}, "积分"), h("b", {}, pick("points", "point", "credit", "credits") ?? "—"),
        h("span", {}, "等级"), h("b", {}, pick("level", "group", "user_group") ?? "—")));
    } catch (e) { box.textContent = e.message; }
  });
}
$("#ov-status").addEventListener("click", (e) => loadAccount(e.currentTarget));
$("#ov-checkin").addEventListener("click", (e) => busy(e.currentTarget, async () => {
  const r = await api("/api/accounts/checkin", { method: "POST" });
  toast(r.demo ? "演示模式：未发送签到请求" : (r.message || "签到成功"));
  await loadSettings(); refresh();
}));

/* ---------- search ---------- */
$("#search-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target, btn = $("button[type=submit]", f);
  busy(btn, async () => {
    const tbody = $("#results");
    tbody.replaceChildren(h("tr", {}, h("td", { colspan: 5, class: "muted center" }, "搜索中…")));
    try {
      const r = await api("/api/resources/search", { method: "POST", body: { query: f.query.value.trim(), media_type: f.media_type.value } });
      state.results = r.resources;
      renderResults();
    } catch (err) {
      tbody.replaceChildren(h("tr", {}, h("td", { colspan: 5, class: "error center" }, err.message)));
    }
  });
});
function renderResults() {
  const tbody = $("#results"), f = $("#search-form");
  if (!state.results.length) { tbody.replaceChildren(h("tr", {}, h("td", { colspan: 5, class: "muted center" }, "没有找到 115 资源"))); return; }
  tbody.replaceChildren(...state.results.map((r) => {
    const over = !r.is_unlocked && (r.unlock_points == null || r.unlock_points > Number(f.max_points.value));
    return h("tr", {},
      h("td", {}, h("div", {}, r.title || r.slug), h("div", { class: "muted small mono" }, r.slug, r.tmdb_id ? ` · TMDB ${r.tmdb_id}` : "")),
      h("td", {}, r.quality || "—"), h("td", {}, r.size || "—"),
      h("td", {}, h("span", { class: `pill ${r.is_unlocked ? "ok" : over ? "warn" : "info"}` }, pointsLabel(r))),
      h("td", { class: "right" }, h("button", { class: over ? "ghost" : "primary", title: over ? "积分超过上限或未知，执行时会被跳过" : "", onclick: (e) => transfer(r, e.currentTarget) }, "转存")));
  }));
}
async function transfer(resource, btn) {
  const f = $("#search-form");
  await busy(btn, async () => {
    const r = await api("/api/jobs", { method: "POST", body: { resource, target_cid: f.target_cid.value.trim(), max_points: Number(f.max_points.value) } });
    toast(r.duplicate ? "同一账号和目录下已有这个任务，未重复添加" : "已加入任务队列", r.duplicate ? "warn" : "ok");
    refresh();
  });
}

/* ---------- subscriptions ---------- */
function renderSubs(list) {
  const tbody = $("#subs");
  if (!list.length) { tbody.replaceChildren(h("tr", {}, h("td", { colspan: 5, class: "muted center" }, "还没有订阅，在右侧新建"))); return; }
  tbody.replaceChildren(...list.map((s) => h("tr", { class: s.enabled ? "" : "disabled" },
    h("td", {}, h("b", {}, s.name), h("div", { class: "muted small" }, `${s.media_type === "tv" ? "剧集" : "电影"} · ${s.query}`)),
    h("td", { class: "small" }, `≤${s.max_points} 分 · CID ${s.target_cid}`, h("br"),
      [s.quality && `画质:${s.quality}`, s.include_keywords && `含:${s.include_keywords}`, s.exclude_keywords && `排除:${s.exclude_keywords}`].filter(Boolean).join(" · ") || "无额外过滤"),
    h("td", { class: "small" }, fmtTime(s.last_run), h("div", { class: "muted" }, s.last_result || "")),
    h("td", {}, h("span", { class: `pill ${s.enabled ? "ok" : "muted"}` }, s.enabled ? (s.auto_transfer ? "自动转存" : "仅检查") : "已停用"),
      h("div", { class: "muted small" }, s.enabled ? `下次 ${s.next_run ? fmtTime(s.next_run) : "即将"}` : "")),
    h("td", { class: "right nowrap" },
      h("button", { class: "ghost sm", onclick: (e) => runSub(s, e.currentTarget), disabled: !s.enabled }, "立即检查"),
      h("button", { class: "ghost sm", onclick: (e) => toggleSub(s, e.currentTarget) }, s.enabled ? "停用" : "启用"),
      h("button", { class: "ghost sm", onclick: () => editSub(s) }, "编辑"),
      h("button", { class: "ghost sm danger", onclick: (e) => deleteSub(s, e.currentTarget) }, "删除")))));
}
function resetSubForm() {
  const f = $("#sub-form"); f.reset();
  const s = state.settings || {};
  f.id.value = ""; f.target_cid.value = s.default_target_cid ?? "0"; f.max_points.value = s.default_max_points ?? 4;
  $("#sub-form-title").textContent = "新建订阅"; $("#sub-reset").hidden = true;
}
function editSub(s) {
  const f = $("#sub-form"); fillForm(f, s);
  $("#sub-form-title").textContent = `编辑：${s.name}`; $("#sub-reset").hidden = false;
  f.name.focus();
}
$("#sub-reset").addEventListener("click", resetSubForm);
$("#sub-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target, d = formData(f), id = d.id; delete d.id;
  busy($("button[type=submit]", f), async () => {
    await api(id ? `/api/subscriptions/${encodeURIComponent(id)}` : "/api/subscriptions", { method: "POST", body: d });
    toast(id ? "订阅已更新" : "订阅已创建，稍后自动检查"); resetSubForm(); refresh();
  });
});
const subBody = (s, patch) => {
  const { id, last_run, last_result, next_run, ...rest } = s; // eslint-disable-line no-unused-vars
  return { ...rest, ...patch };
};
const runSub = (s, btn) => busy(btn, async () => { const r = await api(`/api/subscriptions/${encodeURIComponent(s.id)}/run`, { method: "POST" }); toast(r.message || "已安排检查"); refresh(); });
const toggleSub = (s, btn) => busy(btn, async () => { await api(`/api/subscriptions/${encodeURIComponent(s.id)}`, { method: "POST", body: subBody(s, { enabled: !s.enabled }) }); refresh(); });
const deleteSub = (s, btn) => { if (confirm(`删除订阅「${s.name}」？已创建的任务不受影响。`)) busy(btn, async () => { await api(`/api/subscriptions/${encodeURIComponent(s.id)}`, { method: "DELETE" }); toast("已删除"); refresh(); }); };

/* ---------- jobs ---------- */
function renderJobs(list) {
  const tbody = $("#jobs");
  if (!list.length) { tbody.replaceChildren(h("tr", {}, h("td", { colspan: 6, class: "muted center" }, "暂无任务"))); return; }
  tbody.replaceChildren(...list.map((j) => {
    const [label, kind] = STATUS[j.status] || [j.status, "muted"];
    const retry = j.status === "failed" || j.status === "needs_review";
    return h("tr", {},
      h("td", {}, h("div", {}, j.resource.title || j.resource.slug),
        h("div", { class: "muted small" }, `${pointsLabel(j.resource)} · 上限 ${j.max_points}`, j.subscription_id ? " · 订阅" : " · 手动", j.mode === "demo" ? " · 演示" : "")),
      h("td", { class: "mono small" }, j.target_cid),
      h("td", {}, h("span", { class: `pill ${kind}` }, label)),
      h("td", { class: "small" }, j.message),
      h("td", { class: "small nowrap" }, fmtTime(j.updated_at)),
      h("td", { class: "right" }, retry ? h("button", { class: "ghost sm", onclick: (e) => retryJob(j, e.currentTarget) }, "重试") : ""));
  }));
}
function retryJob(j, btn) {
  let reviewed = false;
  if (j.status === "needs_review") {
    reviewed = confirm("这个任务的外部操作结果不确定（可能已扣积分或已转存）。\n请先在 RE0 与 115 中核对，确认需要重新处理再点「确定」。");
    if (!reviewed) return;
  }
  busy(btn, async () => { await api(`/api/jobs/${encodeURIComponent(j.id)}/retry`, { method: "POST", body: { reviewed } }); toast("已重新排队"); refresh(); });
}

/* ---------- settings ---------- */
async function loadSettings() {
  const s = await api("/api/settings");
  state.settings = s;
  const f = $("#settings-form");
  fillForm(f, s);
  for (const n of ["re0_shared_secret", "tmdb_api_key", "cookie_115"]) f[n].value = "";
  for (const n of ["clear_re0_secret", "clear_tmdb_key", "clear_115_cookie"]) f[n].checked = false;
  const tag = (id, on) => { const t = $(id); t.textContent = on ? "已配置" : "未配置"; t.className = `tag ${on ? "ok" : "muted"}`; };
  tag("#has-secret", s.has_re0_secret); tag("#has-tmdb", s.has_tmdb_key); tag("#has-cookie", s.has_115_cookie);
  const sf = $("#search-form");
  if (!sf.target_cid.value) sf.target_cid.value = s.default_target_cid;
  if (!sf.max_points.value) sf.max_points.value = s.default_max_points;
  if (!$("#sub-form").id.value) resetSubForm();
  applySettingsBadge(s);
}
$("#settings-form").addEventListener("submit", (e) => {
  e.preventDefault();
  const f = e.target, d = formData(f);
  d.re0_base_url = d.re0_base_url.trim(); d.install_id = d.install_id.trim(); d.default_target_cid = d.default_target_cid.trim();
  busy($("button[type=submit]", f), async () => {
    await api("/api/settings", { method: "PUT", body: d });
    toast("设置已保存"); await loadSettings(); refresh();
  });
});
$("#authorize").addEventListener("click", (e) => busy(e.currentTarget, async () => {
  const out = $("#authorize-out");
  try {
    const r = await api("/api/accounts/re0/authorize", { method: "POST" });
    out.replaceChildren("在浏览器中打开以完成授权：", h("a", { href: r.url, target: "_blank", rel: "noopener noreferrer" }, "打开 RE0 授权页"), "。授权后回到概览点「刷新状态」。");
    out.className = "small"; out.hidden = false;
  } catch (err) { out.textContent = err.message; out.className = "small error"; out.hidden = false; }
}));

showAuth(false);
