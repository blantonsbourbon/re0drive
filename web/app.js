"use strict";
const $ = (s, root = document) => root.querySelector(s);
const $$ = s => [...document.querySelectorAll(s)];
const field = (f, n) => f.elements.namedItem(n);
const state = { settings: null, results: [], timer: null, refreshing: false, selected: null, editing: null, authMode: "setup" };
const labels = { overview: "工作台", resources: "发现资源", subscriptions: "我的订阅", tasks: "转存任务", settings: "连接与设置" };
const statuses = { queued: ["排队中", "neutral"], checking: ["检查中", "neutral"], unlocking: ["解锁中", "neutral"], transferring: ["转存中", "neutral"], success: ["已完成", "success"], failed: ["失败", "danger"], skipped: ["已跳过", "neutral"], needs_review: ["待核对", "warning"] };
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") el.className = v;
    else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  for (const c of children.flat()) if (c != null) el.append(c instanceof Node ? c : String(c));
  return el;
}
function fill(f, data) { for (const el of f.elements) if (el.name && el.name in data) { if (el.type === "checkbox") el.checked = !!data[el.name]; else el.value = data[el.name] ?? ""; } }
function values(f) { const out = {}; for (const el of f.elements) if (el.name) out[el.name] = el.type === "checkbox" ? el.checked : el.type === "number" || el.name === "interval_minutes" ? Number(el.value) : el.value.trim(); return out; }
let toastTimer;
function toast(msg, bad = false) { const t = $("#toast"); t.textContent = msg; t.className = `toast${bad ? " error" : ""}`; clearTimeout(toastTimer); toastTimer = setTimeout(() => t.classList.add("hidden"), 5000); }
async function busy(b, fn) { if (b) b.disabled = true; try { return await fn(); } catch (e) { toast(e.message, true); } finally { if (b) b.disabled = false; } }
async function api(path, method = "GET", body) {
  const headers = { Accept: "application/json" };
  if (method !== "GET") headers["X-RE0Drive-Request"] = "1";
  if (body !== undefined) headers["Content-Type"] = "application/json";
  const res = await fetch(path, { method, headers, credentials: "same-origin", body: body === undefined ? undefined : JSON.stringify(body) });
  const data = await res.json().catch(() => null);
  if (res.status === 401 && !path.startsWith("/api/auth/")) { showAuth().catch(() => {}); throw new Error("请先登录"); }
  if (!res.ok) throw new Error(data?.error || `请求失败（HTTP ${res.status}）`);
  return data;
}
function time(ts) { if (!ts) return "等待首次检查"; const d = Date.now() / 1000 - ts; if (d >= 0 && d < 60) return "刚刚"; if (d > 0 && d < 3600) return `${Math.floor(d / 60)} 分钟前`; if (d < 0 && d > -3600) return `${Math.ceil(-d / 60)} 分钟后`; return new Date(ts * 1000).toLocaleString("zh-CN", { hour12: false, month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" }); }
const badge = (text, kind = "neutral") => h("span", { class: `pill ${kind}` }, text);
const button = (text, fn, cls = "text-button green", disabled = false) => h("button", { type: "button", class: cls, onclick: e => busy(e.currentTarget, fn), disabled }, text);
const empty = (title, detail, action) => h("div", { class: "empty-state" }, h("div", { class: "empty-icon" }, "◈"), h("strong", {}, title), h("p", {}, detail), action);
function page(name) { if (!(name in labels)) name = "overview"; $$(".page").forEach(el => el.classList.toggle("hidden", el.id !== `page-${name}`)); $$(".nav-item").forEach(el => el.classList.toggle("active", el.dataset.page === name)); $("#page-label").textContent = labels[name]; history.replaceState(null, "", `#${name}`); }
document.addEventListener("click", e => { const nav = e.target.closest("[data-page]"); if (nav) page(nav.dataset.page); const close = e.target.closest("[data-close]"); if (close) $(`#${close.dataset.close}`).close(); if (e.target.closest('[data-action="new-sub"]')) editSub(); });
window.addEventListener("hashchange", () => page(location.hash.slice(1)));
$("#auth-dialog").addEventListener("cancel", e => e.preventDefault());
async function showAuth() {
  clearInterval(state.timer); state.timer = null;
  $$("dialog[open]").forEach(el => { if (el.id !== "auth-dialog") el.close(); });
  const s = await api("/api/auth/status"); state.authMode = s.configured ? "login" : "setup";
  $("#auth-title").textContent = s.configured ? "欢迎回到工作空间" : "创建你的工作空间";
  $("#auth-description").textContent = s.configured ? "登录后继续管理你的订阅。" : "设置管理员账号，开始管理订阅。";
  field($("#auth-form"), "password").autocomplete = s.configured ? "current-password" : "new-password";
  $("#auth-error").textContent = ""; if (!$("#auth-dialog").open) $("#auth-dialog").showModal();
}
$("#auth-form").addEventListener("submit", e => { e.preventDefault(); busy($('button[type="submit"]', e.target), async () => { try { await api(`/api/auth/${state.authMode}`, "POST", values(e.target)); field(e.target, "password").value = ""; $("#auth-dialog").close(); await start(); } catch (err) { $("#auth-error").textContent = err.message === "请先登录" ? "用户名或密码错误" : err.message; } }); });
$("#logout").addEventListener("click", e => busy(e.currentTarget, async () => { await api("/api/auth/logout", "POST"); await showAuth(); }));
function mode(s) { $("#side-mode").textContent = s.mode === "demo" ? "演示模式" : "中转 API"; $("#demo-banner").classList.toggle("hidden", s.mode !== "demo"); }
async function loadSettings() {
  const s = state.settings = await api("/api/settings"); const f = $("#settings-form"); fill(f, s);
  ["re0_shared_secret", "tmdb_api_key", "cookie_115"].forEach(n => field(f, n).value = "");
  ["clear_re0_secret", "clear_tmdb_key", "clear_115_cookie"].forEach(n => { if (field(f, n)) field(f, n).checked = false; });
  $("#secret-hint").textContent = s.has_re0_secret ? "签名密钥已保存，留空保留原值。" : "尚未配置，可从本地 TgtoDrive 客户端导入。";
  $("#cookie-hint").textContent = s.has_115_cookie ? "Cookie 已保存，留空保留原值。" : "使用你希望接收分享的 115 账号。";
  $("#tmdb-hint").textContent = s.has_tmdb_key ? "TMDB Key 已保存，留空保留原值。" : "未配置时仍可用 TMDB ID 查询资源。";
  $("#cookie-state").textContent = s.has_115_cookie ? "已配置" : "未配置"; $("#cookie-state").className = `pill ${s.has_115_cookie ? "success" : "neutral"}`; mode(s);
}
async function refresh() {
  if (state.refreshing) return; state.refreshing = true;
  try { const d = await api("/api/dashboard"); mode(d.settings);
    $("#stat-sub").textContent = d.stats.subscriptions; $("#stat-pending").textContent = d.stats.pending; $("#stat-success").textContent = d.stats.success; $("#stat-review").textContent = d.stats.review;
    $("#nav-sub-count").textContent = d.subscriptions.length; $("#rule-count").textContent = d.subscriptions.length;
    renderSubs(d.subscriptions); renderActivity(d.activity); renderJobs(d.jobs);
  } finally { state.refreshing = false; }
}
async function start() { await loadSettings(); await refresh(); page(location.hash.slice(1)); clearInterval(state.timer); state.timer = setInterval(() => { if (!document.hidden) refresh().catch(() => {}); }, 5000); }
function renderActivity(list) { $("#activity-list").replaceChildren(...(list.length ? list.slice(0, 5).map(a => h("div", { class: "activity-row" }, h("span", { class: `activity-icon ${a.kind === "success" ? "success" : ""}` }, a.kind === "success" ? "✓" : "↻"), h("div", { class: "activity-text" }, h("strong", {}, a.message), h("small", {}, time(a.created_at))))) : [empty("还没有动态", "创建订阅或搜索资源，开始你的收藏。") ])); }
$("#search-form").addEventListener("submit", e => { e.preventDefault(); busy($('button[type="submit"]', e.target), async () => {
  $("#search-status").textContent = "正在搜索…"; $("#resources-list").replaceChildren(h("div", { class: "loading resource-empty" }, "正在查找资源…"));
  try { const r = await api("/api/resources/search", "POST", { query: $("#search-query").value.trim(), media_type: $("#search-type").value }); state.results = r.resources; renderResults(); }
  catch (err) { state.results = []; $("#search-status").textContent = err.message; $("#resources-list").replaceChildren(empty("暂时无法查询", err.message)); }
}); });
$("#quality-filter").addEventListener("change", renderResults);
function renderResults() {
  const quality = $("#quality-filter").value, list = state.results.filter(r => !quality || `${r.quality} ${r.title}`.includes(quality));
  $("#search-status").textContent = `找到 ${state.results.length} 个资源 · 当前显示 ${list.length} 个${state.settings.mode === "demo" ? " · 示例数据" : ""}`;
  $("#resources-list").replaceChildren(...(list.length ? list.map(r => h("article", { class: "resource-card" }, h("div", { class: "resource-cover" }, h("strong", {}, r.quality.includes("2160") ? "4K" : r.quality.includes("1080") ? "1080P" : "RE0"), badge("115", "light")), h("div", { class: "resource-body" }, h("h3", {}, r.title || r.slug), h("div", { class: "resource-meta" }, h("span", {}, r.quality || "画质未标注"), h("span", {}, r.size || "大小未标注")), h("div", { class: "resource-cost" }, "解锁所需", h("strong", {}, r.is_unlocked ? "已拥有" : r.unlock_points == null ? "积分未知" : `${r.unlock_points} 积分`)), h("div", { class: "resource-actions" }, button("转存到 115 ↗", () => transfer(r), "button primary"), button("订阅此资源", () => editSub(null, r), "button secondary"))))) : [empty("没有匹配的资源", "试试其他片名、TMDB ID 或画质筛选。") ]));
}
function transfer(r) { state.selected = r; $("#transfer-title").textContent = r.title || r.slug; fill($("#transfer-form"), { target_cid: state.settings.default_target_cid, max_points: state.settings.default_max_points }); $("#transfer-dialog").showModal(); }
$("#transfer-form").addEventListener("submit", e => { e.preventDefault(); busy($('button[type="submit"]', e.target), async () => { const r = await api("/api/jobs", "POST", { resource: state.selected, ...values(e.target) }); $("#transfer-dialog").close(); toast(r.duplicate ? "这个资源在目标目录已有任务，请到任务页查看。" : "已加入转存队列"); await refresh(); page("tasks"); }); });
function editSub(s, r) {
  const f = $("#subscription-form"); f.reset(); state.editing = s || null;
  fill(f, s || { id: "", target_cid: state.settings.default_target_cid, max_points: state.settings.default_max_points, name: r ? r.title.split(" · ")[0] : "", query: r ? `tmdb:${r.tmdb_id}` : "", media_type: r?.media_type || "movie", quality: r?.quality.includes("2160") ? "2160" : r?.quality.includes("1080") ? "1080" : "", interval_minutes: 30, auto_transfer: true });
  $("#subscription-title").textContent = s ? "编辑订阅" : "新建订阅"; $("#subscription-dialog").showModal();
}
$("#subscription-form").addEventListener("submit", e => { e.preventDefault(); busy($('button[type="submit"]', e.target), async () => { const d = values(e.target), id = d.id; delete d.id; d.enabled = state.editing?.enabled ?? true; await api(id ? `/api/subscriptions/${id}` : "/api/subscriptions", "POST", d); $("#subscription-dialog").close(); toast(id ? "订阅已更新" : "订阅已创建"); await refresh(); page("subscriptions"); }); });
function renderSubs(list) {
  $("#overview-rules").replaceChildren(...(list.length ? list.slice(0, 4).map(s => h("div", { class: "overview-rule" }, h("span", { class: "rule-avatar" }, s.media_type === "tv" ? "TV" : "M"), h("div", { class: "rule-info" }, h("strong", {}, s.name), h("small", {}, `${s.quality || "不限画质"} · ${s.last_result || "等待首次检查"}`)), badge(s.enabled ? "运行中" : "已停用", s.enabled ? "success" : "neutral"))) : [empty("第一条订阅，等你来建", "把想看的片名交给订阅。", button("＋ 创建订阅", () => editSub(), "button secondary"))]));
  $("#subscription-list").replaceChildren(...(list.length ? list.map(s => h("article", { class: "panel subscription-card" }, h("div", { class: "subscription-head" }, h("h3", {}, s.name), badge(s.enabled ? s.auto_transfer ? "自动转存" : "仅检查" : "已停用", s.enabled ? "success" : "neutral")), h("div", { class: "subscription-query" }, `${s.media_type === "tv" ? "剧集" : "电影"} / ${s.query}`), h("div", { class: "rule-details" }, ...[["版本", s.quality || "不限"], ["积分上限", `${s.max_points} 分`], ["检查间隔", `${s.interval_minutes} 分钟`], ["目标目录", s.target_cid], ["包含", s.include_keywords || "不限"], ["排除", s.exclude_keywords || "无"]].map(([k, v]) => h("div", {}, h("small", {}, k), h("strong", {}, v)))), h("div", { class: "rule-status" }, s.last_result || "等待首次检查", h("br"), `${time(s.last_run)}${s.enabled ? ` · 下次 ${s.next_run ? time(s.next_run) : "即将"}` : ""}`), h("div", { class: "rule-controls" }, button("立即检查", async () => { await api(`/api/subscriptions/${s.id}/run`, "POST"); toast("已安排检查"); await refresh(); }, undefined, !s.enabled), button("编辑", () => editSub(s)), button(s.enabled ? "停用" : "启用", async () => { await api(`/api/subscriptions/${s.id}`, "POST", { ...s, enabled: !s.enabled }); await refresh(); }), button("删除", async () => { if (confirm(`删除订阅「${s.name}」？其尚未执行的任务会跳过。`)) { await api(`/api/subscriptions/${s.id}`, "DELETE"); toast("订阅已删除"); await refresh(); } })))) : [empty("创建你的第一条订阅", "设置片名、版本和目录，定时接收新资源。", button("＋ 新建订阅", () => editSub(), "button primary"))]));
}
function renderJobs(list) {
  $("#jobs-list").replaceChildren(...(list.length ? list.map(j => { const [label, kind] = statuses[j.status] || [j.status, "neutral"];
    return h("tr", {}, h("td", {}, j.resource.title || j.resource.slug, h("small", {}, `${j.mode === "demo" ? "演示任务 · " : ""}上限 ${j.max_points} 分 · ${j.subscription_id ? "订阅" : "手动"}`)), h("td", {}, j.target_cid), h("td", {}, badge(label, kind)), h("td", {}, j.message), h("td", {}, time(j.updated_at), ["failed", "needs_review"].includes(j.status) ? button(j.status === "needs_review" ? "核对后重试" : "重试", async () => { const reviewed = j.status === "needs_review" ? confirm("可能已扣积分或已接收分享。请先核对 RE0 积分和 115 网盘，确认仍需要重试，再点确定。") : false; if (j.status === "needs_review" && !reviewed) return; await api(`/api/jobs/${j.id}/retry`, "POST", { reviewed }); toast("已重新排队"); await refresh(); }) : null)); }) : [h("tr", {}, h("td", { colspan: 5 }, empty("还没有转存任务", "选择一个资源，或创建自动转存订阅。")))]));
}
$("#refresh-jobs").addEventListener("click", e => busy(e.currentTarget, refresh));
$("#settings-form").addEventListener("submit", e => { e.preventDefault(); busy($('button[type="submit"]', e.target), async () => { await api("/api/settings", "PUT", values(e.target)); await loadSettings(); await refresh(); toast("配置已保存"); }); });
$("#authorize").addEventListener("click", e => busy(e.currentTarget, async () => { const r = await api("/api/accounts/re0/authorize", "POST"); const url = new URL(r.url); if (!["https:", "http:"].includes(url.protocol)) throw new Error("授权地址无效"); $("#account-message").replaceChildren("在浏览器中完成授权：", h("a", { href: url.href, target: "_blank", rel: "noopener noreferrer", class: "auth-link" }, "打开 RE0 授权页 ↗"), "。授权后返回并检查状态。"); }));
$("#check-re0").addEventListener("click", e => busy(e.currentTarget, async () => { const r = await api("/api/accounts/re0/status"), el = $("#re0-state"); el.textContent = r.demo ? "演示模式" : r.authorized ? "已授权" : "未授权"; el.className = `pill ${r.authorized ? "success" : "neutral"}`; const u = r.user || {}, name = u.nickname || u.username || u.name || "", points = u.points ?? u.point ?? u.credit ?? u.credits; $("#account-message").textContent = r.demo ? r.message : r.authorized ? `RE0 已授权${name ? ` · ${name}` : ""}${points != null ? ` · ${points} 积分` : ""}` : "请先点击授权 RE0，并在浏览器中完成授权。"; }));
$("#checkin").addEventListener("click", e => busy(e.currentTarget, async () => { const r = await api("/api/accounts/checkin", "POST"); toast(r.demo ? r.message : "签到接口已返回成功"); await refresh(); }));
api("/api/auth/status").then(s => s.authenticated ? start() : showAuth()).catch(e => toast(e.message, true));
