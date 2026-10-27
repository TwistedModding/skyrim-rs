# Records what rust-analyzer completes at points of the sample plugin: python -I scripts/completion.py <workspace> <rust-analyzer.exe> <out.txt>
import json, os, subprocess, sys, threading, time, queue

root, ra, out_path = sys.argv[1], sys.argv[2], sys.argv[3]
src_path = os.path.join(root, "crates", "sample-plugin", "src", "lib.rs")
uri = "file:///" + src_path.replace("\\", "/").replace(":", "%3A", 1)
original = open(src_path, encoding="utf-8").read()

proc = subprocess.Popen([ra], cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
inbox = queue.Queue()

def reader():
    f = proc.stdout
    while True:
        headers = {}
        while True:
            line = f.readline()
            if not line:
                return
            line = line.decode().strip()
            if not line:
                break
            k, v = line.split(":", 1)
            headers[k.lower()] = v.strip()
        inbox.put(json.loads(f.read(int(headers["content-length"]))))

threading.Thread(target=reader, daemon=True).start()
next_id = [0]

def send(msg):
    body = json.dumps(msg).encode()
    proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    proc.stdin.flush()

def request(method, params, timeout=600):
    next_id[0] += 1
    rid = next_id[0]
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
    end = time.time() + timeout
    while time.time() < end:
        try:
            msg = inbox.get(timeout=1)
        except queue.Empty:
            continue
        if msg.get("id") == rid and "method" not in msg:
            return msg
        if "method" in msg and "id" in msg:
            send({"jsonrpc": "2.0", "id": msg["id"], "result": None})
    raise TimeoutError(method)

def notify(method, params):
    send({"jsonrpc": "2.0", "method": method, "params": params})

request("initialize", {"processId": os.getpid(), "rootUri": "file:///" + root.replace("\\", "/"),
    "capabilities": {"textDocument": {"completion": {"completionItem": {"labelDetailsSupport": True}}}, "window": {"workDoneProgress": True}},
    "initializationOptions": {"checkOnSave": False, "cachePriming": {"enable": False}, "completion": {"limit": None}}})
notify("initialized", {})

# Each probe inserts one incomplete line after an anchor line of the source, optionally with the prelude import
# removed, and completes at the end of that line.
PRELUDE = "use skyrim::prelude::*;\n"
probes = [
    ("player.  (player: &'static PlayerCharacter)", "    let player = PlayerCharacter::singleton()?;\n", "    player.", False),
    ("form.  (form: &'static TESForm)", "    if let Some(form) = sre::find_form_by_id(0x8F958) {\n", "        form.", False),
    ("form.  with `use skyrim::prelude::*;` removed", "    if let Some(form) = sre::find_form_by_id(0x8F958) {\n", "        form.", True),
    ("sre::  (the crate root an author imports from)", "fn data_loaded() {\n", "    sre::", False),
]

version = [0]
def complete(anchor, insert, no_prelude):
    base = original.replace(PRELUDE, "") if no_prelude else original
    i = base.index(anchor) + len(anchor)
    text = base[:i] + insert + "\n" + base[i:]
    line = base[:i].count("\n")
    version[0] += 1
    if version[0] == 1:
        notify("textDocument/didOpen", {"textDocument": {"uri": uri, "languageId": "rust", "version": 1, "text": text}})
    else:
        notify("textDocument/didChange", {"textDocument": {"uri": uri, "version": version[0]}, "contentChanges": [{"text": text}]})
    # The first answers come before the workspace is loaded; ask until two answers agree.
    last = None
    for _ in range(120):
        r = request("textDocument/completion", {"textDocument": {"uri": uri}, "position": {"line": line, "character": len(insert)}})
        res = r.get("result") or []
        items = res["items"] if isinstance(res, dict) else res
        labels = sorted((it["label"], json.dumps(it.get("labelDetails"))) for it in items)
        if labels and labels == last:
            return items
        last = labels
        time.sleep(5)
    return []

KIND = {2: "method", 3: "function", 5: "field", 6: "variable", 7: "class", 8: "interface", 9: "module", 10: "property", 13: "enum", 14: "keyword", 15: "snippet", 21: "constant", 22: "struct", 23: "event", 25: "type parameter"}
version_line = subprocess.run([ra, "--version"], capture_output=True, text=True).stdout.strip()
lines = [f"{version_line}, default features, {time.strftime('%Y-%m-%d')}",
         "Source: crates/sample-plugin/src/lib.rs with one incomplete line inserted after the anchor, completion requested at its end.",
         "Columns: kind, label (with the trait a trait method comes from), signature.", ""]
for title, anchor, insert, no_prelude in probes:
    items = complete(anchor, insert, no_prelude)
    kinds = {}
    for it in items:
        k = KIND.get(it.get("kind"), str(it.get("kind")))
        kinds[k] = kinds.get(k, 0) + 1
    lines.append(f"== {title}: {len(items)} items ({', '.join(f'{v} {k}' for k, v in sorted(kinds.items()))})")
    rows = []
    for it in sorted(items, key=lambda it: it["label"]):
        kind = KIND.get(it.get("kind"), str(it.get("kind")))
        detail = (it.get("labelDetails") or {}).get("detail") or ""
        desc = (it.get("labelDetails") or {}).get("description") or it.get("detail") or ""
        rows.append((kind, it["label"], f"{kind}\t{it['label']}{detail}\t{desc}"))
    if len(rows) > 1000:
        # A crate root is summarised: what would betray the raw tables, every module and constant, and a sample.
        def names(pred):
            return [r[1] for r in rows if pred(r)]
        lines.append(f"named raw: {names(lambda r: r[1] == 'raw')}")
        lines.append(f"types ending in _fn: {names(lambda r: r[0] != 'function' and r[1].endswith('_fn'))}")
        lines.append(f"constants: {names(lambda r: r[0] == 'constant')}")
        lines.append(f"modules: {names(lambda r: r[0] == 'module')}")
        lines.append(f"doc(hidden) helpers of lib.rs (Relocation, vtable_entry): {names(lambda r: r[1] in ('Relocation', 'vtable_entry'))}")
        sample = ("PlayerCharacter", "TESDataHandler", "TESForm", "Actor", "FormType", "ActorValue", "MenuOpenCloseEvent", "find_form_by_id", "find_form_by_editor_id_bs_fixed_string", "g_player_ref_ptr")
        lines.append("sample:")
        lines += [r[2] for r in rows if r[1] in sample]
    else:
        lines += [r[2] for r in rows]
    lines.append("")

with open(out_path, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(lines))
request("shutdown", None)
notify("exit", None)
