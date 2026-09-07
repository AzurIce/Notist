//! Server-rendered shell for the preview pipeline debug view. The page embeds
//! one snapshot's [`ModulePipelineRecord`] as JSON; a small dependency-free
//! script renders the stage tabs, the collapsible node trees and the
//! annotation/diagnostic panels client-side.

use notist_service::ModulePipelineRecord;
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

/// Characters that need escaping inside one clean-URL segment when building
/// the back link from module segments (spaces are legal segment characters).
const HREF_SEGMENT_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'[')
    .add(b']')
    .add(b'`')
    .add(b'#')
    .add(b'?');

/// Renders the full pipeline debug page for one module snapshot.
pub fn pipeline_page(module: &str, revision: u64, record: &ModulePipelineRecord) -> String {
    // The site page this module rendered to: the clean URL the chrome's
    // Pipeline link came from, so the debug view can link back.
    let page_url = if record.module_segments.is_empty() {
        "/".to_owned()
    } else {
        let encoded = record
            .module_segments
            .iter()
            .map(|segment| utf8_percent_encode(segment, HREF_SEGMENT_ENCODE_SET).to_string())
            .collect::<Vec<_>>()
            .join("/");
        format!("/{encoded}/")
    };

    let data = serde_json::json!({
        "module": module,
        "revision": revision,
        "record": record,
    });
    // `</` is escaped so a string value can never close the script tag.
    let data_json = serde_json::to_string(&data)
        .unwrap_or_else(|_| "{}".to_owned())
        .replace("</", "<\\/");
    let mut html = String::with_capacity(data_json.len() / 2 + PIPELINE_STYLES.len());
    html.push_str(
        "<!doctype html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>Pipeline · ",
    );
    push_escaped(&mut html, module);
    html.push_str("</title>\n<style>");
    html.push_str(PIPELINE_STYLES);
    html.push_str("</style>\n</head>\n<body>\n<a class=\"back\" href=\"");
    push_escaped(&mut html, &page_url);
    html.push_str("\">← preview&nbsp;<span class=\"crumb\">");
    push_escaped(&mut html, &page_url);
    html.push_str("</span></a>\n<h1>Pipeline <span class=\"module\">");
    push_escaped(&mut html, module);
    html.push_str("</span><span class=\"rev\">snapshot r");
    html.push_str(&revision.to_string());
    html.push_str(
        "</span></h1>\n<nav id=\"tabs\" aria-label=\"Stages\"></nav>\n\
         <div class=\"tools\">\n<input id=\"search\" type=\"search\" \
         placeholder=\"filter nodes (name or string value)\" spellcheck=\"false\">\n\
         <button id=\"expand-all\" type=\"button\">expand all</button>\n\
         <button id=\"collapse-all\" type=\"button\">collapse</button>\n\
         <label class=\"opt\"><input type=\"checkbox\" id=\"show-ranges\" checked> ranges</label>\n\
         <span id=\"stats\" class=\"stats\"></span>\n</div>\n",
    );
    html.push_str("<div id=\"chips\" class=\"chips\"></div>\n<main id=\"view\"></main>\n");
    html.push_str("<script type=\"application/json\" id=\"pipeline-data\">");
    html.push_str(&data_json);
    html.push_str("</script>\n<script>");
    html.push_str(PIPELINE_SCRIPT);
    html.push_str("</script>\n</body>\n</html>\n");
    html
}

fn push_escaped(html: &mut String, input: &str) {
    for character in input.chars() {
        match character {
            '&' => html.push_str("&amp;"),
            '<' => html.push_str("&lt;"),
            '>' => html.push_str("&gt;"),
            '"' => html.push_str("&quot;"),
            value => html.push(value),
        }
    }
}

const PIPELINE_STYLES: &str = r##"
:root { color-scheme: light dark;
  --bg: #ffffff; --fg: #1b1f24; --muted: #68727f; --border: #e3e6ea;
  --panel: #f6f7f9; --accent: #2563eb; --mark: #fde047; --danger: #dc2626; }
@media (prefers-color-scheme: dark) {
  :root { --bg: #10131a; --fg: #dfe3e8; --muted: #8b93a1; --border: #272d38;
    --panel: #171c26; --accent: #82b1ff; --mark: #6b5510; --danger: #f87171; } }
* { box-sizing: border-box; }
body { margin: 0; font: 14px/1.5 system-ui, sans-serif; background: var(--bg); color: var(--fg); }
header { position: sticky; top: 0; z-index: 3; background: var(--bg);
  border-bottom: 1px solid var(--border); padding: .65rem 1rem .55rem; }
h1 { font-size: .95rem; margin: 0 0 .55rem; display: flex; gap: .6rem; align-items: baseline; }
a.back { display: inline-block; margin-bottom: .5rem; font-size: .82rem;
  color: var(--muted); text-decoration: none; }
a.back:hover { color: var(--accent); }
a.back .crumb { font-family: ui-monospace, SFMono-Regular, monospace; }
h1 .module { font-family: ui-monospace, SFMono-Regular, monospace; color: var(--accent); }
h1 .rev { color: var(--muted); font-size: .78rem; font-weight: 400; }
#tabs { display: flex; gap: .3rem; flex-wrap: wrap; }
#tabs button { font: inherit; font-size: .84rem; padding: .22rem .75rem; cursor: pointer;
  border: 1px solid var(--border); border-radius: 999px; background: transparent; color: var(--fg); }
#tabs button:hover { border-color: var(--accent); }
#tabs button.active { background: var(--accent); border-color: var(--accent); color: var(--bg); }
#tabs button.active .n { opacity: .75; }
#tabs .n { opacity: .6; font-size: .74rem; margin-left: .3rem; }
#tabs kbd { font-family: inherit; font-size: .68rem; opacity: .5; margin-right: .3rem; }
.tools { display: flex; gap: .5rem; align-items: center; margin-top: .55rem; flex-wrap: wrap; }
#search { flex: 1; min-width: 14rem; font: inherit; font-size: .85rem; color: var(--fg);
  background: var(--panel); border: 1px solid var(--border); border-radius: 6px; padding: .28rem .6rem; }
.tools button, .tools label { font: inherit; font-size: .8rem; color: var(--fg);
  background: transparent; border: 1px solid var(--border); border-radius: 6px; padding: .26rem .6rem; }
.tools label { display: inline-flex; gap: .35rem; align-items: center; cursor: pointer; }
.stats { color: var(--muted); font-size: .78rem; margin-left: auto; padding-right: .4rem; }
.chips { display: flex; gap: .35rem; flex-wrap: wrap; padding: .55rem 1rem 0; }
.chips:empty { display: none; }
.chip { font-family: ui-monospace, monospace; font-size: .72rem; color: var(--muted);
  border: 1px solid var(--border); border-radius: 999px; padding: .04rem .55rem;
  background: var(--panel); cursor: pointer; }
.chip:hover { border-color: var(--accent); color: var(--fg); }
main { padding: .7rem 1rem 4rem; }
.tree { font-family: ui-monospace, SFMono-Regular, monospace; font-size: .82rem; }
.row { display: flex; align-items: baseline; gap: .45rem; padding: .1rem .35rem;
  border-radius: 4px; min-width: max-content; }
.row:hover { background: var(--panel); }
.caret { flex: none; width: 1em; text-align: center; cursor: pointer; color: var(--muted);
  user-select: none; }
.caret.leaf { visibility: hidden; }
.kids { margin-left: .95rem; padding-left: .6rem; border-left: 1px dotted var(--border); }
.kids.hidden { display: none; }
.arggroup > .row > .name { color: var(--muted); font-style: italic; font-weight: 400; }
.name { font-weight: 600; }
.name.ns-core { color: #2563eb; } .name.ns-html { color: #9333ea; }
.name.ns-plugin { color: #059669; }
.name.ns-bare { color: #d97706; outline: 1px dashed color-mix(in srgb, currentColor 55%, transparent);
  outline-offset: 2px; border-radius: 3px; }
@media (prefers-color-scheme: dark) {
  .name.ns-core { color: #82b1ff; } .name.ns-html { color: #c99cff; }
  .name.ns-plugin { color: #4ade80; } .name.ns-bare { color: #fbbf24; } }
.args { color: var(--muted); }
.arg .k { color: var(--muted); }
.arg .v { color: var(--fg); }
.rng { color: var(--muted); font-size: .72rem; opacity: .8; }
body.hide-ranges .rng { display: none; }
mark { background: var(--mark); color: var(--bg); border-radius: 2px; }
.empty { color: var(--muted); font-style: italic; }
.note { color: var(--muted); font-size: .8rem; margin: .3rem 0 .6rem; }
iframe { width: 100%; height: 62vh; border: 1px solid var(--border); border-radius: 6px; background: #fff; }
details.src { margin-top: .8rem; }
details.src summary { cursor: pointer; color: var(--muted); }
details.src pre, pre.raw { overflow: auto; max-height: 60vh; font-family: ui-monospace, monospace;
  font-size: .78rem; background: var(--panel); border: 1px solid var(--border);
  border-radius: 6px; padding: .6rem .8rem; }
table { border-collapse: collapse; font-size: .82rem; }
th, td { border: 1px solid var(--border); padding: .18rem .55rem; text-align: left; vertical-align: top; }
th { color: var(--muted); font-weight: 600; background: var(--panel); }
td.mono { font-family: ui-monospace, monospace; font-size: .76rem; }
.diag { color: var(--danger); font-family: ui-monospace, monospace; font-size: .8rem; }
.panel-title { font-size: .85rem; font-weight: 600; margin: .2rem 0 .5rem; }
"##;

const PIPELINE_SCRIPT: &str = r##"
(function () {
  "use strict";
  var DATA = JSON.parse(document.getElementById("pipeline-data").textContent);
  var RECORD = DATA.record;
  var STAGES = [
    { key: "lowered", label: "Lowered" },
    { key: "forest", label: "Reduced" },
    { key: "tree", label: "Shaped" },
    { key: "projected", label: "Projected" },
    { key: "__html", label: "HTML" },
    { key: "__annotations", label: "Annotations" },
    { key: "__diagnostics", label: "Diagnostics" },
  ];
  var state = { stage: "tree", filter: "", showRanges: true, expand: "default" };

  function walk(list, visit) {
    for (var i = 0; i < list.length; i++) {
      visit(list[i]);
      walk(list[i].children || [], visit);
      var args = list[i].args || [];
      for (var a = 0; a < args.length; a++) {
        var value = args[a][1];
        if (value && value.Stream) walk(value.Stream, visit);
      }
    }
  }

  function nodeStats(nodes) {
    var count = 0;
    var levels = 0;
    function measure(list, level) {
      for (var i = 0; i < list.length; i++) {
        count += 1;
        if (level > levels) levels = level;
        measure(list[i].children || [], level + 1);
        var args = list[i].args || [];
        for (var a = 0; a < args.length; a++) {
          var value = args[a][1];
          if (value && value.Stream) measure(value.Stream, level + 1);
        }
      }
    }
    measure(nodes, 1);
    return { count: count, depth: levels };
  }

  function nameCounts(nodes) {
    var counts = new Map();
    walk(nodes, function (node) {
      counts.set(node.name, (counts.get(node.name) || 0) + 1);
    });
    return counts;
  }

  function matches(node, needle) {
    if (node.name.toLowerCase().indexOf(needle) !== -1) return true;
    var args = node.args || [];
    for (var i = 0; i < args.length; i++) {
      var value = args[i][1];
      if (value && value.String && value.String.toLowerCase().indexOf(needle) !== -1) return true;
    }
    return false;
  }

  function subtreeMatches(node, needle) {
    if (matches(node, needle)) return true;
    var children = node.children || [];
    for (var i = 0; i < children.length; i++) {
      if (subtreeMatches(children[i], needle)) return true;
    }
    var args = node.args || [];
    for (var a = 0; a < args.length; a++) {
      var value = args[a][1];
      if (value && value.Stream) {
        for (var s = 0; s < value.Stream.length; s++) {
          if (subtreeMatches(value.Stream[s], needle)) return true;
        }
      }
    }
    return false;
  }

  function namespaceClass(name) {
    if (name.indexOf("core::") === 0) return "ns-core";
    if (name.indexOf("html::") === 0) return "ns-html";
    if (name.indexOf("::") !== -1) return "ns-plugin";
    // A bare name is sugar before reduction, or a call nobody handles:
    // either way it is the thing to spot in every stage.
    return "ns-bare";
  }

  function el(tag, className, text) {
    var element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = text;
    return element;
  }

  function truncate(text, limit) {
    return text.length > limit ? text.slice(0, limit - 1) + "\u2026" : text;
  }

  function truncatedSpan(text, limit) {
    var span = el("span", "v", truncate(text, limit));
    if (text.length > limit) span.title = text;
    return span;
  }

  function argValueSpan(value) {
    var kind = Object.keys(value)[0];
    var inner = value[kind];
    switch (kind) {
      case "String": return truncatedSpan(JSON.stringify(inner), 66);
      case "Int": case "Float": case "Bool": return el("span", "v", String(inner));
      case "None": return el("span", "v", "()");
      case "Stream": return el("span", "v", "stream(" + inner.length + ")");
      case "Array": return el("span", "v", "array(" + inner.length + ")");
      default: return truncatedSpan(kind + " " + JSON.stringify(inner), 48);
    }
  }

  function highlightName(name, needle) {
    if (!needle) return;
    var text = name.textContent;
    var index = text.toLowerCase().indexOf(needle);
    if (index === -1) return;
    name.textContent = text.slice(0, index);
    name.appendChild(el("mark", "", text.slice(index, index + needle.length)));
    name.appendChild(document.createTextNode(text.slice(index + needle.length)));
  }

  // expand: "all" renders every kid eagerly; "none" keeps everything
  // collapsed; "default" opens the first two levels (and anything when
  // filtering, since filter matches must be visible).
  function buildNode(node, depth, needle, expand) {
    var wrap = el("div", "node");
    var row = el("div", "row");
    var kids = el("div", "kids hidden");

    var children = node.children || [];
    var argStreams = [];
    var args = node.args || [];
    for (var i = 0; i < args.length; i++) {
      var value = args[i][1];
      if (value && value.Stream && value.Stream.length) {
        argStreams.push([args[i][0], value.Stream]);
      }
    }
    var hasKids = children.length > 0 || argStreams.length > 0;

    var caret = el("span", hasKids ? "caret" : "caret leaf", hasKids ? "▸" : "·");
    row.appendChild(caret);

    var name = el("span", "name " + namespaceClass(node.name));
    name.textContent = node.name;
    highlightName(name, needle);
    row.appendChild(name);

    for (var a = 0; a < args.length; a++) {
      if (args[a][1] && args[a][1].Stream) continue; // streams render as kid groups
      var arg = el("span", "arg");
      arg.appendChild(el("span", "k", args[a][0] + "="));
      arg.appendChild(argValueSpan(args[a][1]));
      row.appendChild(arg);
    }
    if (state.showRanges && node.range) {
      row.appendChild(el("span", "rng", node.range.start + "–" + node.range.end));
    }

    var rendered = false;
    function renderKids() {
      if (rendered) return;
      rendered = true;
      argStreams.forEach(function (stream) {
        var group = el("div", "node arggroup");
        var groupRow = el("div", "row");
        groupRow.appendChild(el("span", "caret leaf", "·"));
        groupRow.appendChild(el("span", "name", "arg " + stream[0]));
        group.appendChild(groupRow);
        var groupKids = el("div", "kids");
        stream[1].forEach(function (child) {
          groupKids.appendChild(buildNode(child, depth + 2, needle, expand));
        });
        group.appendChild(groupKids);
        kids.appendChild(group);
      });
      children.forEach(function (child) {
        kids.appendChild(buildNode(child, depth + 1, needle, expand));
      });
    }

    if (hasKids) {
      caret.addEventListener("click", function () {
        var hidden = kids.classList.toggle("hidden");
        caret.textContent = hidden ? "▸" : "▾";
        if (!hidden) renderKids();
      });
      var open = expand === "all" || (expand === "default" && (depth < 2 || needle));
      if (open) {
        caret.textContent = "▾";
        kids.classList.remove("hidden");
        renderKids();
      }
    }

    wrap.appendChild(row);
    wrap.appendChild(kids);
    return wrap;
  }

  function renderChips(nodes) {
    var chips = document.getElementById("chips");
    chips.textContent = "";
    var counts = Array.from(nameCounts(nodes).entries())
      .sort(function (a, b) { return b[1] - a[1] || (a[0] < b[0] ? -1 : 1); })
      .slice(0, 14);
    counts.forEach(function (entry) {
      var chip = el("button", "chip", entry[0] + " ×" + entry[1]);
      chip.addEventListener("click", function () {
        var search = document.getElementById("search");
        search.value = entry[0];
        onSearch(entry[0]);
      });
      chips.appendChild(chip);
    });
  }

  function renderStage() {
    var view = document.getElementById("view");
    view.textContent = "";
    document.getElementById("chips").textContent = "";
    var stats = document.getElementById("stats");
    var stage = state.stage;

    if (stage === "__html") {
      view.appendChild(el("p", "note", "the fragment exactly as build/preview write it"));
      var frame = document.createElement("iframe");
      frame.setAttribute("srcdoc", RECORD.html);
      view.appendChild(frame);
      var details = el("details", "src");
      details.appendChild(el("summary", "", "escaped html source"));
      details.appendChild(el("pre", "raw", RECORD.html));
      view.appendChild(details);
      stats.textContent = RECORD.html.length + " bytes";
      return;
    }
    if (stage === "__annotations") {
      view.appendChild(el("p", "panel-title", "side annotation table (range → entries)"));
      if (!RECORD.annotations.length) {
        view.appendChild(el("p", "empty", "(no annotations)"));
      } else {
        var table = el("table");
        var head = el("tr");
        ["range", "id", "classes", "tags", "properties"].forEach(function (label) {
          head.appendChild(el("th", "", label));
        });
        table.appendChild(head);
        RECORD.annotations.forEach(function (annotation) {
          var row = el("tr");
          row.appendChild(el("td", "mono", annotation.start + "–" + annotation.end));
          row.appendChild(el("td", "mono", annotation.id || ""));
          row.appendChild(el("td", "mono", annotation.classes.join(" ")));
          row.appendChild(el("td", "mono", annotation.tags.join(" ")));
          row.appendChild(el("td", "mono", JSON.stringify(annotation.properties)));
          table.appendChild(row);
        });
        view.appendChild(table);
      }
      stats.textContent = RECORD.annotations.length + " entries";
      return;
    }
    if (stage === "__diagnostics") {
      if (!RECORD.diagnostics.length) {
        view.appendChild(el("p", "empty", "(no evaluation diagnostics)"));
      }
      RECORD.diagnostics.forEach(function (diagnostic) {
        var range = diagnostic.range;
        var start = range ? range.start : 0;
        var end = range ? range.end : 0;
        view.appendChild(el("p", "diag", start + "–" + end + "  " + diagnostic.message));
      });
      stats.textContent = RECORD.diagnostics.length + " entries";
      return;
    }

    var nodes = RECORD[stage];
    var needle = state.filter.trim().toLowerCase();
    var visible = needle
      ? nodes.filter(function (node) { return subtreeMatches(node, needle); })
      : nodes;
    if (!nodes.length) {
      view.appendChild(el("p", "empty", "(empty forest)"));
    } else if (needle && !visible.length) {
      view.appendChild(el("p", "empty", "(no node matches \u201C" + state.filter.trim() + "\u201D)"));
    } else {
      var tree = el("div", "tree");
      visible.forEach(function (node) {
        tree.appendChild(buildNode(node, 1, needle || null, state.expand));
      });
      view.appendChild(tree);
      if (needle) {
        view.appendChild(el("p", "note", visible.length + " of " + nodes.length + " root trees match"));
      }
      renderChips(nodes);
    }
    // When a transformation stage is a no-op for this module, say so — two
    // identical trees otherwise read as a rendering bug rather than as
    // "nothing to project here".
    if (!needle) {
      if (stage === "forest" && sameJson(RECORD.lowered, RECORD.forest)) {
        view.appendChild(el(
          "p", "note",
          "Identical to Lowered: no registered call needed reduction in this module.",
        ));
      }
      if (stage === "projected" && sameJson(RECORD.tree, RECORD.projected)) {
        view.appendChild(el(
          "p", "note",
          "Identical to Shaped: every node is core::/html:: (pass-through). "
            + "Projection only rewrites scopes, plugin elements and unresolved "
            + "names; core elements become HTML in the serializer.",
        ));
      }
    }
    var statsValue = nodeStats(nodes);
    stats.textContent = statsValue.count + " nodes · depth " + statsValue.depth;
  }

  function sameJson(left, right) {
    return JSON.stringify(left) === JSON.stringify(right);
  }

  function setStage(key) {
    state.stage = key;
    var buttons = document.querySelectorAll("#tabs button");
    for (var i = 0; i < buttons.length; i++) {
      buttons[i].classList.toggle("active", buttons[i].dataset.stage === key);
    }
    renderStage();
  }

  function onSearch(value) {
    state.filter = value;
    renderStage();
  }

  function init() {
    var tabs = document.getElementById("tabs");
    STAGES.forEach(function (stage, index) {
      var button = document.createElement("button");
      button.type = "button";
      button.dataset.stage = stage.key;
      button.appendChild(el("kbd", "", index + 1 + " "));
      button.appendChild(document.createTextNode(stage.label + " "));
      var count = stage.key.indexOf("__") === 0
        ? RECORD[stage.key.slice(2)].length
        : nodeStats(RECORD[stage.key]).count;
      button.appendChild(el("span", "n", String(count)));
      button.addEventListener("click", function () { setStage(stage.key); });
      tabs.appendChild(button);
    });

    document.getElementById("search").addEventListener("input", function (event) {
      onSearch(event.target.value);
    });
    document.getElementById("expand-all").addEventListener("click", function () {
      state.expand = "all";
      renderStage();
    });
    document.getElementById("collapse-all").addEventListener("click", function () {
      state.expand = "none";
      renderStage();
    });
    document.getElementById("show-ranges").addEventListener("change", function (event) {
      state.showRanges = event.target.checked;
      document.body.classList.toggle("hide-ranges", !event.target.checked);
    });
    document.addEventListener("keydown", function (event) {
      if (event.target.tagName === "INPUT") {
        if (event.key === "Escape") {
          event.target.value = "";
          onSearch("");
        }
        return;
      }
      var index = parseInt(event.key, 10);
      if (index >= 1 && index <= STAGES.length) setStage(STAGES[index - 1].key);
      if (event.key === "/") {
        event.preventDefault();
        document.getElementById("search").focus();
      }
    });

    setStage("tree");
  }

  init();
})();
"##;
