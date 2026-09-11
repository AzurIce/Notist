#!/usr/bin/env python3
"""Score vsearch-bench output against labeled targets.

Usage: score.py RESULTS.json [MORE.json ...]   (arms are merged by name)

Relevance: a hit is relevant when its path matches the target file and its
heading chain is on the target's ancestor/descendant axis (target.chain is a
prefix of hit.chain, or hit.chain is a prefix of target.chain).
strict  = hit.chain == target.chain or hit.chain startswith target.chain + "/"
lenient = strict or hit.chain is an ancestor of target.chain
"""
import json
import sys

# id -> (path, full target chain)
T = {
 "q01": ("04-world/annotation.not", "Annotation Table/继承与有效环境"),
 "q02": ("04-world/annotation.not", "Annotation Table/坐标：节点序列区间"),
 "q03": ("04-world/annotation.not", "Annotation Table/条目与挂载位置"),
 "q04": ("04-world/model.not", "数据模型/ModulePath"),
 "q05": ("04-world/model.not", "数据模型/Item 树"),
 "q06": ("04-world/model.not", "数据模型/Module 求值单元与 ModuleResult"),
 "q07": ("04-world/reference.not", "Reference 与 RefTarget/ItemId 匹配与消歧"),
 "q08": ("04-world/import.not", "Import/依赖图与环语义"),
 "q09": ("04-world/boundary.not", "Vault 边界与发现/Notist.toml marker"),
 "q10": ("04-world/model.not", "数据模型/virtual module 与资源"),
 "q11": ("04-world/reference.not", "Reference 与 RefTarget/引用图"),
 "q12": ("04-world/reference.not", "Reference 与 RefTarget/两个寻址世界：结构地址与词法绑定"),
 "q13": ("03-language/evaluation.not", "Evaluation/Lowering：Call 的来源"),
 "q14": ("03-language/evaluation.not", "Evaluation/名称环境"),
 "q15": ("03-language/evaluation.not", "Evaluation/规约：声明式不动点"),
 "q16": ("03-language/functions.not", "Built-in Functions/link"),
 "q17": ("05-cli/daemon.not", "Daemon and Analyzer Views/为什么需要 daemon"),
 "q18": ("05-cli/daemon.not", "Daemon and Analyzer Views/Daemon 持有什么"),
 "q19": ("05-cli/daemon.not", "Daemon and Analyzer Views/形态：每 Vault 一个 daemon"),
 "q20": ("05-cli/search.not", "Search Semantics and Retrieval Index/Index 就绪与一致性"),
 "q21": ("05-cli/search.not", "Search Semantics and Retrieval Index/可检索字段"),
 "q22": ("05-cli/search.not", "Search Semantics and Retrieval Index/向量与 semantic mode 的门槛"),
 "q23": ("05-cli/inspect/read.not", "Inspect Read: 带属性注解的读取/分解规则"),
 "q24": ("05-cli/inspect/refs.not", "Inspect Refs: 穿越查询区域边界的引用"),
 "q25": ("05-cli/README.not", "Notist CLI: Command Design and Reference/Selector 与 Scope"),
 "q26": ("05-cli/README.not", "Notist CLI: Command Design and Reference/Error 与退出状态"),
 "q27": ("05-cli/build.not", "Build, Preview, and HTML Target/Local preview"),
 "q28": ("05-cli/build.not", "Build, Preview, and HTML Target/HTML target 序列化/锚点与引用链接"),
 "q29": ("05-cli/README.not", "Notist CLI: Command Design and Reference/向量化属于 Vault 运行时状态"),
 "q30": ("06-lsp/README.not", "LSP/文本同步"),
 "q31": ("06-lsp/README.not", "LSP/坐标与编码"),
 "q32": ("06-lsp/README.not", "LSP/诊断发布"),
 "q33": ("07-plugins/abi.not", "Plugin ABI/postcard 帧契约"),
 "q34": ("07-plugins/abi.not", "Plugin ABI/运行时与装载"),
 "q35": ("07-plugins/README.not", "Plugin System/信任边界"),
 "q36": ("01-intro.not", "认识 Notist/编译管线"),
 "q37": ("05-cli/README.not", "Notist CLI: Command Design and Reference/Validate"),
 "q38": ("01-intro.not", "认识 Notist/标注"),
 "q39": ("02-cheatsheet.not", "标题/Code 速查"),
 "q40": ("05-cli/inspect.not", "CLI Inspect: Agent Query and Navigation/refs"),
 "g1": ("05-cli/search.not", "Search Semantics and Retrieval Index/检索单元与索引"),
 "g2": ("05-cli/daemon.not", "Daemon and Analyzer Views/组合根：WorkspaceSnapshot 捕获 semantic environment"),
 "g3": ("05-cli/search.not", "Search Semantics and Retrieval Index/Index 就绪与一致性"),
 "g4": ("07-plugins/abi.not", "Plugin ABI/postcard 帧契约"),
 "g5": ("ai/2026-08-28 core wasm 插件后端统一与 wasmi 落地.not", "core wasm 插件后端统一与 wasmi 落地"),
 "g6": ("ai/2026-08-31 区域属性投影与 inspect info 命令.not", "区域属性投影与 inspect info 命令"),
}


def relevant(hit, target):
    path, chain = target
    if hit["path"] != path:
        return 0
    hc, tc = hit["chain"], chain
    strict = hc == tc or hc.startswith(tc + "/")
    lenient = strict or tc.startswith(hc + "/") or hc == tc
    return 2 if strict else (1 if lenient else 0)


def recall_at(hits, target, k, level):
    for hit in hits[:k]:
        rel = relevant(hit, target)
        if rel == 2 or (level == "lenient" and rel >= 1):
            return 1
    return 0


def first_relevant_rank(hits, target, level):
    for i, hit in enumerate(hits, 1):
        rel = relevant(hit, target)
        ok = rel == 2 or (level == "lenient" and rel >= 1)
        if ok:
            return i
    return None


def main(paths):
    arms = []
    for results_path in paths:
        for arm in json.load(open(results_path)):
            if arm["arm"] not in [a["arm"] for a in arms]:
                arms.append(arm)
    kinds = ["concept", "paraphrase", "identifier"]
    rows = []
    for arm in arms:
        by_kind = {k: [] for k in kinds}
        for q in arm["queries"]:
            target = T.get(q["id"])
            if target is None:
                continue
            by_kind.setdefault(q["kind"], []).append((q, target))
        stats = {"arm": arm["arm"], "chunks": arm["chunk_count"], "build_s": round(arm["build_ms"] / 1000)}
        for level in ("strict", "lenient"):
            for k in (5, 10):
                vals = {k2: sum(recall_at(q["hits"], t, k, level) for q, t in qs) / len(qs)
                        for k2, qs in by_kind.items() if qs}
                all_qs = [item for qs in by_kind.values() for item in qs]
                vals["ALL"] = sum(recall_at(q["hits"], t, k, level) for q, t in all_qs) / len(all_qs)
                stats[f"{level}@{k}"] = vals
        for level in ("strict", "lenient"):
            all_qs = [item for qs in by_kind.values() for item in qs]
            ranks = [(first_relevant_rank(q["hits"], t, level),) for q, t in all_qs]
            rr = [1.0 / r[0] for r in ranks if r[0]]
            stats[f"mrr_{level}@10"] = round(sum(rr) / len(all_qs), 3)
            stats[f"hit_any_{level}"] = sum(1 for r in ranks if r[0]) / len(all_qs)
        qms = [q["ms"] for q in arm["queries"]]
        stats["avg_query_ms"] = round(sum(qms) / len(qms))
        rows.append(stats)

    kinds_order = ["concept", "paraphrase", "identifier", "ALL"]
    for level in ("strict", "lenient"):
        print(f"\n===== {level.upper()} =====")
        hdr = f"{'arm':<16}{'chunks':>7}{'build_s':>9}{'q_ms':>7}"
        for k in kinds_order:
            hdr += f"  R@5-{k[:4]:>4}" if k != "ALL" else "   R@5-ALL"
        for k in kinds_order:
            hdr += f"  R@10-{k[:4]:>4}" if k != "ALL" else "  R@10-ALL"
        hdr += "   MRR@10  hit%"
        print(hdr)
        for s in rows:
            line = f"{s['arm']:<16}{s['chunks']:>7}{s['build_s']:>9}{s['avg_query_ms']:>7}"
            for k in kinds_order:
                line += f"  {s[f'{level}@5'][k]:>9.2f}" if k != "ALL" else f"  {s[f'{level}@5'][k]:>8.2f}"
            for k in kinds_order:
                line += f"  {s[f'{level}@10'][k]:>9.2f}" if k != "ALL" else f"  {s[f'{level}@10'][k]:>8.2f}"
            line += f"  {s[f'mrr_{level}@10']:>7.3f}  {s[f'hit_any_{level}'] * 100:>4.0f}%"
            print(line)

    print("\n===== per-query misses (strict@10), by arm =====")
    names = [a["arm"] for a in arms]
    print(f"{'id':<6}" + "".join(f"{n:<16}" for n in names))
    if arms:
        for q in arms[0]["queries"]:
            marks = []
            for n in names:
                a = next(a for a in arms if a["arm"] == n)
                qq = next(x for x in a["queries"] if x["id"] == q["id"])
                target = T.get(q["id"])
                ok = any(relevant(h, target) == 2 for h in qq["hits"])
                marks.append("." if ok else "X")
            if "X" in marks:
                print(f"{q['id']:<6}" + "".join(f"{m:<16}" for m in marks))


if __name__ == "__main__":
    main(sys.argv[1:])
