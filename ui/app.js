"use strict";
const $ = (id) => document.getElementById(id);
const escapeHtml = (value) =>
  String(value ?? "").replace(
    /[&<>"']/g,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
const e = escapeHtml;
const invoke = (op, payload = {}) =>
  window.__TAURI__.core.invoke("operate", { op, payload });
const states = {
  Running: "运行中",
  Stopped: "已停止",
  Error: "需处理",
  Paused: "已断网",
};
let state,
  view = "profiles",
  activity = "launches",
  busy = false,
  filter = "",
  submitAction,
  cancelAction;
const time = (epoch) =>
  epoch
    ? new Date(epoch * 1000).toLocaleString("zh-CN", {
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      })
    : "—";
function button(action, label, id = "", cls = "") {
  return (
    '<button type="button" data-action="' +
    e(action) +
    '" data-id="' +
    e(id) +
    '" class="' +
    e(cls) +
    '">' +
    e(label) +
    "</button>"
  );
}
function input(key, label, value = "", type = "text", extra = "") {
  return (
    "<label>" +
    e(label) +
    '<input name="' +
    key +
    '" type="' +
    type +
    '" value="' +
    e(value) +
    '" ' +
    extra +
    "></label>"
  );
}
function options(rows, id) {
  return rows
    .map(
      (x) =>
        '<option value="' +
        e(x.id) +
        '" ' +
        (x.id === id ? "selected" : "") +
        ">" +
        e(x.name) +
        "</option>",
    )
    .join("");
}
function nodeOptions(id) {
  return options(
    state.nodes.map((n) => ({
      ...n,
      name:
        n.name +
        " · " +
        (state.sources.find((s) => s.id === n.source)?.name || ""),
    })),
    id,
  );
}
function notice(text = "", bad = false) {
  $("notice").textContent = text;
  $("notice").hidden = !text;
  $("notice").classList.toggle("bad", bad);
}
function modal(title, fields, submit, fn, cancelFn) {
  if ($("dialog").open) $("dialog").close();
  $("dialogTitle").textContent = title;
  $("fields").innerHTML = fields;
  $("submit").textContent = submit;
  $("submit").disabled = false;
  $("formError").hidden = true;
  submitAction = fn;
  cancelAction = cancelFn;
  $("dialog").showModal();
}
async function cancelModal() {
  if (busy) return;
  const fn = cancelAction;
  cancelAction = null;
  $("dialog").close();
  if (fn) await fn();
}
$("cancel").onclick = cancelModal;
$("dismiss").onclick = cancelModal;
$("dialog").oncancel = (ev) => {
  ev.preventDefault();
  cancelModal();
};
$("form").onsubmit = async (ev) => {
  ev.preventDefault();
  if (busy) return;
  const fn = submitAction;
  const data = Object.fromEntries(new FormData(ev.target));
  busy = true;
  $("submit").disabled = true;
  $("formError").hidden = true;
  try {
    const after = await fn(data);
    $("dialog").close();
    cancelAction = null;
    await sync();
    if (typeof after === "function") after();
  } catch (err) {
    $("formError").textContent = String(err);
    $("formError").hidden = false;
  } finally {
    busy = false;
    $("submit").disabled = false;
  }
};
let metricProfile = "",
  report = { samples: [], n: 0 };
async function sync() {
  state = await invoke("snapshot");
  if (view === "history" && activity === "metrics")
    report = await invoke("metrics", { id: metricProfile });
  draw();
}
function empty(title, description, action, label) {
  return (
    '<div class="empty"><img src="icon.png" alt=""><h3>' +
    e(title) +
    "</h3><p>" +
    e(description) +
    "</p>" +
    (action ? button(action, label, "", "primary") : "") +
    "</div>"
  );
}
const info = (label, value) =>
  '<div><small class="detail">' + e(label) + "</small>" + e(value) + "</div>";
function profileCards() {
  const items = state.profiles.filter((p) =>
    p.name.toLowerCase().includes(filter.toLowerCase()),
  );
  if (!items.length) return empty("没有匹配的配置文件", "可以尝试其他名称。");
  return items.map((p) => {
    const live = p.state === "Running";
    const active = live || p.state === "Paused";
    const node = state.nodes.find((n) => n.id === p.node);
    const ipLabel = p.ip && !live ? "上次出口 IP" : "出口 IP";
    const zone = p.auto && !p.ip ? "" : p.env.zone;
    const zoneLabel = p.auto && p.ip && !live ? "上次出口时区" : "时区";
    const extra = [
      active
        ? button("switch", p.state === "Paused" ? "重新校验" : "切换节点", p.id)
        : button("edit", "编辑", p.id),
      button("clone", "复制配置", p.id),
      active ? "" : button("delete", "移入回收目录", p.id, "danger"),
    ].join("");
    return `<article class="profile-row">
      <div class="cardhead">
        <div class="identity">
          <div class="avatar" aria-hidden="true">${e(Array.from(p.name)[0])}</div>
          <div><h3>${e(p.name)}</h3><small>${e(node?.name || "绑定节点已移除")} · ${p.browser === "obscura" ? "Obscura" : "Chrome"}${zone && p.env.locale ? " · " + e(p.env.locale) : ""}</small></div>
        </div>
        <span class="pill ${e(p.state)}">${e(states[p.state])}</span>
      </div>
      <dl class="network-fields">
        <div><dt>${ipLabel}</dt><dd${p.ip ? "" : ' class="pending"'}>${p.ip ? e(p.ip).replaceAll(":", ":<wbr>") : "待启动校验"}</dd></div>
        <div><dt>${zoneLabel}</dt><dd${zone ? "" : ' class="pending"'}>${e(zone || "待出口校验")}</dd></div>
      </dl>
      <div class="profile-controls">
        ${active ? button("stop", "停止", p.id) : button("start", "启动", p.id, "launch")}
        <details class="more" name="profile-actions" data-profile="${e(p.id)}"><summary aria-label="${e(p.name)}的更多操作">更多操作</summary><div class="rowactions">${extra}</div></details>
      </div>
      ${p.metrics ? '<small class="profile-metrics">' + e(Math.round(p.metrics.memory)) + ' MB · CPU ' + e(p.metrics.cpu.toFixed(1)) + '% · ' + e(p.metrics.tabs) + ' 个标签页</small>' : ""}
      ${p.error ? '<p class="error">' + e(p.error) + "</p>" : ""}
    </article>`;
  }).join("");
}
function profilePage() {
  if (!state.profiles.length)
    return empty(
      "创建第一个配置文件",
      "每个配置文件使用自己的浏览器数据、代理节点和地区环境。",
      state.nodes.length ? "new" : "addSource",
      state.nodes.length ? "创建配置文件" : "先添加节点",
    );
  return '<div class="toolbar profile-toolbar"><label class="search-box"><input id="search" type="search" placeholder="搜索配置文件" aria-label="搜索配置文件" value="' +
    e(filter) + '"></label></div><div id="cards">' + profileCards() + "</div>";
}
function nodesPage() {
  if (!state.sources.length)
    return empty(
      "还没有节点来源",
      "可导入 Clash / Mihomo 订阅、导入 YAML 文件，或手动填写代理节点。",
      "addSource",
      "添加节点来源",
    );
  return state.sources
    .map((s) => {
      const nodes = state.nodes.filter((n) => n.source === s.id);
      return (
        '<article class="panel"><div class="row"><div><h3>' +
        e(s.name) +
        "</h3><small>" +
        e(s.remote ? "订阅" : "本地导入") +
        " · " +
        nodes.length +
        " 个节点 · " +
        time(s.updated) +
        '</small></div><div class="rowactions">' +
        (s.remote
          ? button("refresh", "刷新订阅", s.id)
          : button("replace", "更新节点", s.id)) +
        button("removeSource", "移除", s.id, "danger") +
        '</div></div><details><summary>查看节点</summary><div class="scroll"><table><tbody>' +
        nodes
          .map(
            (n) =>
              "<tr><td>" +
              e(n.name) +
              "</td><td>" +
              e(n.kind.toUpperCase()) +
              "</td></tr>",
          )
          .join("") +
        "</tbody></table></div></details></article>"
      );
    })
    .join("");
}
function historyPage() {
  const launch = activity === "launches",
    rows = state[activity];
  let html =
    '<div class="toolbar"><div>' +
    button("launches", "启动记录", "", "tab " + (launch ? "selected" : "")) +
    button(
      "metrics",
      "资源采样",
      "",
      "tab " + (activity === "metrics" ? "selected" : ""),
    ) +
    button(
      "events",
      "事件",
      "",
      "tab " + (activity === "events" ? "selected" : ""),
    ) +
    "</div></div>";
  if (activity === "metrics") {
    html +=
      '<label>查看 Profile<select id="metricProfile"><option value="">全部 Profile</option>' +
      options(state.profiles, metricProfile) +
      "</select></label>";
    html +=
      '<div class="panel"><h3>近 30 天的启动耗时</h3><div class="details">' +
      info("已就绪的启动次数", report.n) +
      info("P50", report.n ? report.p50 + " ms" : "—") +
      info("P95", report.n ? report.p95 + " ms" : "—") +
      "</div></div>";
    if (!report.samples.length)
      return (
        html + empty("暂无资源采样", "运行中的 Profile 每 10 秒记录一次。")
      );
    return (
      html +
      '<div class="panel scroll"><table><thead><tr><th>采样时间</th><th>Profile</th><th>进程树内存</th><th>CPU</th><th>标签页</th></tr></thead><tbody>' +
      report.samples
        .map(
          (r) =>
            "<tr>" +
            [
              time(r.ts),
              state.profiles.find((p) => p.id === r.profile)?.name || "已移除",
              Math.round(r.memory_mb) + " MB",
              r.cpu.toFixed(1) + "%",
              r.tabs,
            ]
              .map((c) => "<td>" + e(c) + "</td>")
              .join("") +
            "</tr>",
        )
        .join("") +
      "</tbody></table></div>"
    );
  }
  if (!rows.length)
    return (
      html + empty("暂无记录", "启动浏览器或处理节点变更后，记录会出现在这里。")
    );
  const headers = launch
    ? ["开始时间", "Profile", "出口 IP", "校验 / 就绪", "结束原因"]
    : ["时间", "Profile", "事件"];
  return (
    html +
    '<div class="panel scroll"><table><thead><tr>' +
    headers.map((x) => "<th>" + x + "</th>").join("") +
    "</tr></thead><tbody>" +
    rows
      .map((row) => {
        const name =
          state.profiles.find((p) => p.id === row.profile)?.name ||
          (row.profile ? "已移除的 Profile" : "管理器");
        const cells = launch
          ? [
              time(row.started),
              name,
              row.ip || "—",
              (row.verify_ms ?? "—") + " / " + (row.ready_ms ?? "—") + " ms",
              row.end_reason || "运行中",
            ]
          : [time(row.ts), name, row.detail];
        return (
          "<tr>" + cells.map((c) => "<td>" + e(c) + "</td>").join("") + "</tr>"
        );
      })
      .join("") +
    "</tbody></table></div>"
  );
}
function trashPanel() {
  if (!state.trash?.length) return "";
  return (
    '<div class="panel"><h3>回收目录</h3><p class="muted">浏览器数据保留 7 天，到期自动清理。</p>' +
    state.trash
      .map(
        (p) =>
          '<div class="row"><span>' +
          e(p.name) +
          "<br><small>" +
          time(p.deleted) +
          '</small></span><div class="rowactions">' +
          button("restore", "恢复", p.id) +
          button("purge", "立即清除", p.id, "danger") +
          "</div></div>",
      )
      .join("") +
    "</div>"
  );
}
function runtimePage() {
  return (
    '<div class="panel"><h3>浏览器与地理位置库</h3><p class="muted">可以复用本机 Chrome，也可以安装此版本固定的 Google Chrome 稳定版。</p><div class="row"><span>Chrome<br><small>' +
    e(state.chrome ? state.chromePath : "尚未安装") +
    "</small></span>" +
    button("chrome", "安装并使用固定版本") + (state.managed ? button("systemChrome","优先使用系统 Chrome") : "") +
    '</div><div class="row"><span>Obscura 0.2.7<br><small>' + (state.obscura ? "已安装" : "首次使用时下载") + '</small></span>' +
    button("obscura", state.obscura ? "校验或修复" : "安装 Obscura") +
    '</div><div class="row"><span>离线城市库<br><small>IP Geolocation by DB-IP.com · CC BY 4.0 · ' +
    (state.geo ? "已安装" : "待下载") +
    "</small></span>" +
    button("geo", state.geo ? "重新下载" : "下载城市库") +
    '</div></div><div class="panel"><h3>出口校验</h3><p class="muted">每次启动和切换都会经所选节点访问回显服务。主备服务均失败时会关闭该 Profile 的网络。</p><div class="row"><code>' +
    e(state.echo) +
    "</code>" +
    button("settings", "编辑回显地址") +
    "</div></div>" +
    trashPanel()
  );
}
function draw() {
  if (!state) return;
  const names = {
    profiles: "配置文件",
    nodes: "节点与订阅",
    history: "运行记录",
    runtime: "运行时",
  };
  const running = state.profiles.filter((p) =>
    ["Running", "Paused"].includes(p.state),
  ).length;
  const desc = {
    profiles: state.profiles.length + " 个配置文件 · 已占用 " + running + " / 5 个槽位",
    nodes: state.sources.length + " 个来源 · " + state.nodes.length + " 个节点",
    history: "查看启动耗时、出口校验和故障事件",
    runtime: "管理本机浏览器、离线数据和出口校验服务",
  };
  $("title").textContent = names[view];
  $("subtitle").textContent = desc[view];
  $("coreDot").classList.toggle("on", state.core);
  $("coreLabel").textContent = state.core ? "代理内核运行中" : "代理内核待机";
  document
    .querySelectorAll("[data-nav]")
    .forEach((b) => {
      const selected = b.dataset.nav === view;
      b.classList.toggle("active", selected);
      if (selected) b.setAttribute("aria-current", "page");
      else b.removeAttribute("aria-current");
    });
  $("actions").innerHTML =
    view === "profiles"
      ? button("new", "＋ 新建配置文件", "", "primary")
      : view === "nodes"
        ? button("addSource", "＋ 添加来源", "", "primary")
        : "";
  const expanded = new Set(Array.from(document.querySelectorAll(".more[open]"), (menu) => menu.dataset.profile));
  $("content").innerHTML = {
    profiles: profilePage,
    nodes: nodesPage,
    history: historyPage,
    runtime: runtimePage,
  }[view]();
  document.querySelectorAll(".more").forEach((menu) => { menu.open = expanded.has(menu.dataset.profile); });
  if ($("metricProfile"))
    $("metricProfile").onchange = async (ev) => {
      metricProfile = ev.target.value;
      await sync();
    };
  if ($("search"))
    $("search").oninput = (ev) => {
      filter = ev.target.value;
      $("cards").innerHTML = profileCards();
    };
}
const languageNames = new Intl.DisplayNames(["zh-CN"], { type: "language" });
const localePresets = [
  "zh-CN", "zh-HK", "zh-TW", "en-GB", "en-US", "en-CA", "en-AU", "en-SG",
  "ja-JP", "ko-KR", "de-DE", "fr-FR", "fr-CA", "es-ES", "it-IT", "pt-BR",
].map((id) => ({ id, name: languageNames.of(id) + " · " + id }));
const zonePresets = [
  ["Europe/London", "伦敦"], ["Asia/Shanghai", "北京、上海"], ["Asia/Hong_Kong", "香港"],
  ["Asia/Taipei", "台北"], ["Asia/Tokyo", "东京"], ["Asia/Seoul", "首尔"],
  ["Asia/Singapore", "新加坡"], ["America/New_York", "纽约"], ["America/Chicago", "芝加哥"],
  ["America/Denver", "丹佛"], ["America/Los_Angeles", "洛杉矶"], ["America/Toronto", "多伦多"],
  ["Europe/Paris", "巴黎"], ["Europe/Berlin", "柏林"], ["Australia/Sydney", "悉尼"],
  ["UTC", "协调世界时"],
].map(([id, city]) => ({ id, name: city + " · " + id }));
function environmentPicker(key, label, rows, value) {
  const selected = rows.some((row) => row.id === value) ? value : "";
  return '<label>' + label + '<select name="' + key + 'Choice">' +
    options([...rows, { id: "", name: "自定义…" }], selected) + '</select></label>' +
    input(key, "自定义" + label, value, "text", 'spellcheck="false"');
}
function selectedEnvironment(form) {
  const read = (key) => form[key + "Choice"] || form[key]?.trim() || "";
  const zone = read("zone"), tag = read("locale");
  let locale;
  try {
    [locale] = Intl.getCanonicalLocales(tag);
    if (!locale || !Intl.DateTimeFormat.supportedLocalesOf([locale]).length) throw Error();
  } catch {
    throw Error("请输入有效且受支持的语言标签，例如 en-GB。");
  }
  try {
    const date = new Intl.DateTimeFormat(locale, {
      timeZone: zone, dateStyle: "full", timeStyle: "short",
    });
    return { zone, locale, preview: date.format(new Date()) };
  } catch {
    throw Error("请输入有效的城市时区，例如 Europe/London。");
  }
}
function editProfile(p) {
  if (!state.nodes.length) {
    notice("先添加一个代理节点，然后就可以创建 Profile。");
    view = "nodes";
    draw();
    sourceForm();
    return;
  }
  const env = p?.env || {
    zone: "Asia/Shanghai",
    locale: "zh-CN",
    lat: 31.23,
    lng: 121.47,
  };
  modal(
    p ? "编辑配置文件" : "新建配置文件",
    input("name", "名称", p?.name, "text", 'required maxlength="100"') +
      '<label>浏览器内核<select name="browser" id="browserKind">' +
      options([{id:"chrome",name:"Google Chrome"},{id:"obscura",name:"Obscura · 交互预览"}], p?.browser || "chrome") +
      '</select></label><p id="engineHint" class="hint">单页浏览，支持点击、输入和滚动。首次使用下载约 65 MiB。与 Chrome 分开保存登录状态；部分网页格式和交互可能不同。</p>' +
      '<label>绑定节点<select name="node">' +
      nodeOptions(p?.node) +
      '</select></label><label>地区环境<select name="mode" id="mode"><option value="auto">根据出口自动推导</option><option value="manual">手动设置</option></select></label><fieldset id="manual" aria-label="手动地区设置"><div class="pair">' +
      '<div>' + environmentPicker("locale", "语言", localePresets, env.locale) + '</div><div>' +
      environmentPicker("zone", "时区", zonePresets, env.zone) +
      '</div></div><p id="localPreview" class="hint" role="status"></p><div class="pair">' +
      input("lat", "纬度", env.lat, "number", 'step="any" min="-90" max="90"') +
      input(
        "lng",
        "经度",
        env.lng,
        "number",
        'step="any" min="-180" max="180"',
      ) +
      '</div></fieldset><p class="hint" id="geoHint">首次使用自动环境时会下载并校验离线城市库。</p>',
    "保存",
    async (d) => {
      const selected = d.mode === "manual" ? selectedEnvironment(d) : env;
      await invoke("save", {
        id: p?.id || "",
        name: d.name,
        node: d.node,
        browser: d.browser,
        auto: d.mode === "auto",
        env: {
          zone: selected.zone,
          locale: selected.locale,
          lat: Number(d.lat),
          lng: Number(d.lng),
        },
      });
      notice("Profile 已保存。");
    },
  );
  const showEngineHint = () => { $("engineHint").hidden = $("browserKind").value !== "obscura"; };
  $("browserKind").onchange = showEngineHint;
  showEngineHint();
  $("mode").value = p?.auto === false ? "manual" : "auto";
  const change = () => {
    $("manual").hidden = $("mode").value === "auto";
    $("manual").disabled = $("manual").hidden;
    $("geoHint").hidden = !$("manual").hidden;
  };
  if (p?.auto && p.ip) {
    $("geoHint").textContent = "最近一次出口校验 · " + env.locale + " · " + env.zone;
  }
  const form = $("form");
  const preview = () => {
    for (const key of ["locale", "zone"]) {
      const custom = form.elements[key];
      custom.disabled = Boolean(form.elements[key + "Choice"].value);
      custom.closest("label").hidden = custom.disabled;
    }
    if ($("manual").hidden) return;
    try {
      const result = selectedEnvironment(Object.fromEntries(new FormData(form)));
      $("localPreview").textContent = "当地时间预览 · " + result.preview;
    } catch (err) {
      $("localPreview").textContent = err.message;
    }
  };
  $("manual").oninput = preview;
  $("mode").onchange = () => { change(); preview(); };
  change();
  preview();
}
function sourceForm(existing) {
  const modes = existing ? "" : '<option value="url">订阅 URL</option>';
  modal(
    existing ? "更新节点来源" : "添加节点来源",
    input(
      "name",
      "来源名称",
      existing?.name,
      "text",
      'required maxlength="100"',
    ) +
      '<label>导入方式<select id="sourceMode" name="mode"><option value="manual">手动节点</option>' +
      modes +
      '<option value="yaml">Clash / Mihomo YAML</option></select></label><div id="sourceFields"></div>',
    "保存来源",
    async (d) => {
      let yaml = d.yaml || "";
      if (d.mode === "manual") {
        const n = {
          name: d.nodeName || d.name,
          type: d.protocol,
          server: d.host.trim(),
          port: Number(d.port),
        };
        if (d.user) n.username = d.user;
        if (d.password) n.password = d.password;
        if (d.tls) n.tls = true;
        yaml = JSON.stringify(n);
      }
      await invoke("source", {
        id: existing?.id || "",
        name: d.name,
        url: d.mode === "url" ? d.url : "",
        yaml,
      });
      notice("节点来源已保存。");
    },
  );
  function change() {
    const mode = $("sourceMode").value;
    $("sourceFields").innerHTML =
      mode === "url"
        ? input(
            "url",
            "订阅地址",
            "",
            "url",
            'required autocomplete="off" placeholder="https://…"',
          )
        : mode === "yaml"
          ? '<label>从文件读取<input id="yamlFile" type="file" accept=".yaml,.yml,.json,text/plain"></label><label>节点配置<textarea name="yaml" id="yaml" rows="8" required spellcheck="false" placeholder="proxies:"></textarea></label>'
          : input("nodeName", "节点名称（可与来源同名）") +
            '<div class="pair"><label>协议<select name="protocol"><option value="http">HTTP / HTTPS</option><option value="socks5">SOCKS5</option></select></label>' +
            input(
              "port",
              "端口",
              "8080",
              "number",
              'required min="1" max="65535"',
            ) +
            "</div>" +
            input("host", "服务器", "", "text", "required") +
            '<div class="pair">' +
            input("user", "用户名（可选）", "", "text", 'autocomplete="off"') +
            input(
              "password",
              "密码（可选）",
              "",
              "password",
              'autocomplete="new-password"',
            ) +
            '</div><label><span><input type="checkbox" name="tls" class="check"> 使用 TLS 连接代理</span></label>';
    if ($("yamlFile"))
      $("yamlFile").onchange = async (ev) => {
        const f = ev.target.files[0];
        if (f && f.size <= 8388608) $("yaml").value = await f.text();
        else {
          $("formError").textContent = "请选择小于 8 MB 的 YAML 文件";
          $("formError").hidden = false;
        }
      };
  }
  if (existing) $("sourceMode").value = "yaml";
  $("sourceMode").onchange = change;
  change();
}
function regionDialog(r, op, payload) {
  modal(
    "出口地区已变化",
    "<p>上次出口为 " +
      e(r.previous + " · " + r.previousZone) +
      "，本次为 " +
      e(r.country + " · " + r.newZone) +
      "（" +
      e(r.ip) +
      '）。确认后会再次校验。</p><label>地区环境<select name="decision"><option value="update">使用出口环境（如需更改时区会重启浏览器）</option><option value="keep">保留原环境并转为手动</option></select></label>',
    "确认并继续",
    async (d) => {
      const again = await invoke(op, {
        ...payload,
        decision: d.decision,
        confirmed: r.confirmed,
      });
      return again.confirm
        ? () => regionDialog(again, op, payload)
        : () => notice("出口已重新校验。");
    },
    async () => {
      if (op === "switch") await invoke("stop", { id: payload.id });
      await sync();
      notice("已取消此次校验。");
    },
  );
}
async function run(op, payload, msg) {
  busy = true;
  notice(msg + "…");
  try {
    const res = await invoke(op, payload);
    await sync();
    if (res.confirm) {
      notice("出口地区变更，等待确认。");
      regionDialog(res, op, payload);
    } else notice("已完成。");
  } catch (err) {
    notice(String(err), true);
    await sync();
  } finally {
    busy = false;
  }
}
document.addEventListener("keydown", (ev) => {
  if (ev.key !== "Escape") return;
  const menu = document.querySelector(".more[open]");
  if (menu) {
    menu.open = false;
    menu.querySelector("summary").focus();
    ev.preventDefault();
  }
});
document.addEventListener("click", async (ev) => {
  const b = ev.target.closest("button");
  const menu = document.querySelector(".more[open]");
  if (menu && (b || !menu.contains(ev.target))) menu.open = false;
  if (!b) return;
  if (b.dataset.nav) {
    if (busy) return;
    view = b.dataset.nav;
    notice();
    draw();
    return;
  }
  const a = b.dataset.action,
    id = b.dataset.id;
  if (!a || busy || !state) return;
  const p = state.profiles.find((p) => p.id === id),
    s = state.sources.find((s) => s.id === id);
  if (a === "new" || a === "edit") return editProfile(p);
  if (a === "addSource" || a === "replace") return sourceForm(s);
  if (["launches", "metrics", "events"].includes(a)) {
    activity = a;
    await sync();
    return;
  }
  if (a === "switch")
    return modal(
      "切换代理节点",
      '<label>新的节点<select name="node">' +
        nodeOptions(p.node) +
        '</select></label><p class="hint">切换时会关闭此 Profile 的旧连接，新的出口通过校验后恢复访问。</p>',
      "切换并校验",
      async (d) => {
        const payload = { id, node: d.node };
        const res = await invoke("switch", payload);
        return res.confirm
          ? () => regionDialog(res, "switch", payload)
          : () => notice("节点已切换并通过校验。");
      },
    );
  if (a === "delete" || a === "removeSource")
    return modal(
      a === "delete" ? "移入回收目录" : "移除节点来源",
      "<p>" +
        e(
          a === "delete"
            ? "「" + p.name + "」的浏览器数据将保留 7 天。"
            : "「" + s.name + "」的节点将被移除，使用这些节点的会话将停止。",
        ) +
        "</p>",
      "确认移除",
      async () => {
        await invoke(a, { id });
      },
    );
  if (a === "purge")
    return modal(
      "永久清除浏览器数据",
      "<p>此操作会删除所选 Profile 的本地浏览器数据，清除后无法恢复。</p>",
      "永久清除",
      async () => {
        await invoke("purge", { id });
      },
    );
  if (a === "settings")
    return modal(
      "出口回显地址",
      input("echo", "主服务", state.echo, "url", "required") +
        input("fallback", "备用服务（可留空）", state.fallback, "url"),
      "保存",
      async (d) => {
        await invoke("settings", d);
      },
    );
  await run(
    a,
    { id },
    {
      start: "正在校验出口并启动浏览器",
      stop: "正在停止浏览器",
      clone: "正在复制配置",
      geo: "正在下载并校验城市库",
      chrome: "正在安装并验证 Chrome",
      obscura: "正在校验 Obscura 内核",
      refresh: "正在刷新订阅并复查受影响会话",
    }[a] || "正在处理",
  );
});
(async () => {
  try {
    await sync();
    await window.__TAURI__.event.listen("harbor-phase", (ev) => {
      if (busy) notice(String(ev.payload) + "…");
    });
    await window.__TAURI__.event.listen("harbor-updated", () => {
      if (!busy && !$("dialog").open && document.activeElement?.id !== "search" && !document.activeElement?.closest(".more[open]"))
        sync().catch((err) => notice(String(err), true));
    });
  } catch (err) {
    notice("管理器连接失败：" + err, true);
  }
})();
