#!/usr/bin/env python3
"""Exhaustive audit of the Mojang bedrock-protocol-docs JSON meta-schema.

Walks every JSON file under vendor/bedrock-protocol-docs/json and reports:
  * every key that can appear on a schema node (with counts),
  * every unique value of the enumerable keys (type, x-underlying-type, ...),
  * every structural "shape" (sorted key-set) of a schema node,
  * the JSON value kinds seen for each key.

Usage: python scripts/audit_schema.py [json_dir] [--shapes]
"""
import json, os, sys, collections

args = [a for a in sys.argv[1:] if not a.startswith("--")]
root = args[0] if args else os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "vendor", "bedrock-protocol-docs", "json")
root = os.path.abspath(root)
if os.name == "nt" and not root.startswith(chr(92) * 2):
    root = chr(92) * 2 + "?" + chr(92) + root  # long-path prefix
show_shapes = "--shapes" in sys.argv

NAME_MAPS = {"properties", "definitions", "$defs", "patternProperties", "dependencies"}
FREE = {"title", "description", "$id", "$ref", "$schema", "x-minecraft-version", "required", "enum",
        "x-enum-binary-value", "default", "const", "examples"}
key_count = collections.Counter()
key_kinds = collections.defaultdict(collections.Counter)
values = collections.defaultdict(collections.Counter)
shapes = collections.Counter()
shape_example = {}
meta_keys = collections.Counter()
ctx_count = collections.Counter()

def kind(v):
    if isinstance(v, bool): return "bool"
    if isinstance(v, int): return "int"
    if isinstance(v, float): return "float"
    if isinstance(v, str): return "str"
    if isinstance(v, list): return "list[" + ",".join(sorted({kind(x) for x in v})) + "]"
    if isinstance(v, dict): return "object"
    return "null"

def walk(node, ctx, fname):
    if not isinstance(node, dict):
        ctx_count["non-object schema node (%s) in %s" % (kind(node), ctx)] += 1
        return
    ctx_count[ctx] += 1
    shape = tuple(sorted(node.keys()))
    shapes[shape] += 1
    shape_example.setdefault(shape, fname)
    for k, v in node.items():
        key_count[k] += 1
        key_kinds[k][kind(v)] += 1
        if k in NAME_MAPS:
            for name, sub in v.items():
                walk(sub, k, fname)
        elif k == "$metaProperties":
            for mk, mv in v.items():
                meta_keys[mk + " : " + kind(mv)] += 1
        elif k in ("items", "additionalProperties", "not", "if", "then", "else", "contains", "propertyNames"):
            if isinstance(v, list):
                for sub in v: walk(sub, k + "[]", fname)
            elif isinstance(v, dict):
                walk(v, k, fname)
            else:
                values[k][repr(v)] += 1
        elif k in ("oneOf", "anyOf", "allOf", "prefixItems"):
            for sub in v: walk(sub, k, fname)
        elif k not in FREE:
            if isinstance(v, list):
                for x in v: values[k][json.dumps(x, sort_keys=True)] += 1
            else:
                values[k][json.dumps(v, sort_keys=True)] += 1

files = sorted(f for f in os.listdir(root) if f.endswith(".json"))
for f in files:
    with open(os.path.join(root, f), encoding="utf-8") as fh:
        walk(json.load(fh), "<root>", f)

print("files:", len(files))
print("\n== node contexts ==")
for k, c in ctx_count.most_common(): print("%6d  %s" % (c, k))
print("\n== keys (count, value kinds) ==")
for k, c in key_count.most_common():
    print("%6d  %-28s %s" % (c, k, dict(key_kinds[k])))
print("\n== $metaProperties keys ==")
for k, c in meta_keys.most_common(): print("%6d  %s" % (c, k))
print("\n== unique values ==")
for k in sorted(values):
    vs = values[k]
    print("-- %s (%d unique)" % (k, len(vs)))
    for v, c in vs.most_common(60 if len(vs) > 60 else None): print("%6d  %s" % (c, v))
    if len(vs) > 60: print("        ... (%d more)" % (len(vs) - 60))
print("\n== node shapes: %d ==" % len(shapes))
if show_shapes:
    for s, c in shapes.most_common(): print("%6d  %s   e.g. %s" % (c, list(s), shape_example[s]))
