"""Generate Rust bindings (src/classes.rs, src/enums.rs) from Roblox's API dump.

usage: python3 gen/gen.py API-Dump.json
The dump is published at
https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/API-Dump.json
"""
import json, os, re, sys

dump = json.load(open(sys.argv[1]))
here = os.path.dirname(os.path.abspath(__file__))
out_dir = os.path.join(here, "..", "src")

KEYWORDS = set("""as async await break const continue crate dyn else enum extern false fn for if impl in let loop
match mod move mut pub ref return self Self static struct super trait true type unsafe use where while abstract
become box do final macro override priv typeof unsized virtual yield try gen""".split())
# names reserved by the crate / Rust traits on every class
RESERVED = {"clone": "clone_instance", "cast": "cast_", "handle": "handle_", "as_instance": "as_instance_",
            "from_handle": "from_handle_", "eq": "eq_", "ne": "ne_", "deref": "deref_", "new": "new_", "get": "get_"}

def snake(name):
    for a, b in (("CFrame", "Cframe"), ("UDim2", "Udim2"), ("UDim", "Udim"), ("GUID", "Guid"), ("URL", "Url"),
                 ("UI", "Ui"), ("ID", "Id"), ("XR", "Xr"), ("VR", "Vr"), ("HTTP", "Http"), ("JSON", "Json")):
        name = re.sub(a + r"(?=[A-Z0-9_]|$)", b, name)
    s = re.sub(r"([A-Z]+)([A-Z][a-z])", r"\1_\2", name)
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", s)
    s = re.sub(r"[^A-Za-z0-9_]", "_", s).lower()
    if s[0].isdigit():
        s = "_" + s
    return s

def ident(name):
    s = snake(name)
    s = RESERVED.get(s, s)
    return s + "_" if s in KEYWORDS else s

def rust_str(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'

classes = {c["Name"]: c for c in dump["Classes"]}
enums = {e["Name"]: e for e in dump["Enums"]}

def type_ident(name):
    return re.sub(r"[^A-Za-z0-9_]", "_", name)

# ---- type mapping -------------------------------------------------------------------------
# A spec: (rust param type, extern params [(type, expr from x)], Luau constructor from params,
#          rust return type, extern return type, Luau return expression from `v`, rust from `r`,
#          event FromValue type)
class T:
    def __init__(self, **k):
        self.__dict__.update(k)

def leafs(n):
    return n

VALUE = T(rin="impl IntoValue", ext=[("u32", "x.raw()")], pre="let x = x.into_value();", n=1, lin="rt_hget({0})",
          rout="Value", eout="u32", lout="rt_href(v)", conv="unsafe { Value::from_raw(r) }", ev="Value")
TUPLE_IN = T(rin="&[Value]", ext=[("u32", "x.raw()")], pre="let x = Value::pack(x);", n=1,
             lin="table.unpack(rt_hget({0}), 1, rt_hget({0}).n)", rout=None, ev="Value")

def spec(t, setter=False):
    s = spec_exact(t, setter)
    if s is not None:
        return s
    if isinstance(t, dict) and t["Category"] == "Group" and t["Name"] == "Tuple":
        return TUPLE_IN
    if isinstance(t, dict) and t["Category"] == "Primitive" and t["Name"] == "null":
        return None
    return VALUE

def spec_exact(t, setter=False):
    cat, name = t["Category"], t["Name"]
    num = lambda rt: T(rin=rt, ext=[(rt, "x")], n=1, lin="{0}", rout=rt, eout=rt, lout="v", conv="r", ev=rt)
    if cat == "Primitive":
        if name == "bool": return num("bool")
        if name in ("float", "double"): return num("f64")
        if name == "int": return num("i32")
        if name == "int64":
            return T(rin="i64", ext=[("i64", "x")], n=2,
                     lin="(({1} >= 2147483648 and ({1} - 4294967296) or {1}) * 4294967296 + {0})",
                     rout="i64", eout="i64", lout="v % 4294967296, math.floor(v / 4294967296) % 4294967296",
                     conv="r", ev="i64")
        if name == "string":
            return T(rin="&str", ext=[("*const u8", "x.as_ptr()"), ("usize", "x.len()")], n=2, lin="rt_str({0}, {1})",
                     rout="String", eout="RawStr", lout="rt_str_out(v)", conv="r.into_string()", ev="String")
        if name == "null":
            return None
        return None
    if cat == "DataType":
        if name in ("ContentId", "ProtectedString"):
            return spec({"Category": "Primitive", "Name": "string"})
        dt = {
            "Vector3": (3, "Vector3.new({0}, {1}, {2})", "v.X, v.Y, v.Z"),
            "Vector2": (2, "Vector2.new({0}, {1})", "v.X, v.Y"),
            "Color3": (3, "Color3.new({0}, {1}, {2})", "v.R, v.G, v.B"),
            "UDim": (2, "UDim.new({0}, {1})", "v.Scale, v.Offset"),
            "UDim2": (4, "UDim2.new({0}, {1}, {2}, {3})", "v.X.Scale, v.X.Offset, v.Y.Scale, v.Y.Offset"),
            "CFrame": (12, "CFrame.new(" + ", ".join("{%d}" % i for i in range(12)) + ")", "v:GetComponents()"),
            "Rect": (4, "Rect.new({0}, {1}, {2}, {3})", "v.Min.X, v.Min.Y, v.Max.X, v.Max.Y"),
            "NumberRange": (2, "NumberRange.new({0}, {1})", "v.Min, v.Max"),
        }
        if name in dt:
            n, lin, lout = dt[name]
            ev = name if name in ("Vector3", "Vector2", "Color3", "UDim2", "CFrame") else "Value"
            return T(rin=name, ext=[(name, "x")], n=n, lin=lin, rout=name, eout=name, lout=lout, conv="r", ev=ev)
        if name == "Instances":
            return T(rin=None, rout="Vec<Instance>", eout="u32", lout="rt_href(v)", conv="instances(r)", ev="Value")
        if name == "BrickColor":
            return T(rin="BrickColor", ext=[("u32", "x.0")], n=1, lin="BrickColor.new({0})", rout="BrickColor",
                     eout="u32", lout="v.Number", conv="BrickColor(r)", ev="Value")
        return None
    if cat == "Enum":
        if name not in enums:
            return None
        e = "enums::" + type_ident(name)
        return T(rin=e, ext=[("u32", "x.0")], n=1, lin="rt_enum(Enum." + name + ", {0})", rout=e, eout="u32",
                 lout="v.Value", conv=e + "(r)", ev=e)
    if cat == "Class":
        if name not in classes:
            return None
        c = type_ident(name)
        return T(rin=("Option<&" + c + ">") if setter else ("&" + c), ext=[("u32", "rt::opt_id(x)" if setter else "x.handle().id()")],
                 n=1, lin="rt_hget({0})", rout="Option<" + c + ">", eout="u32", lout="rt_href(v)",
                 conv="unsafe { Handle::from_raw_opt(r) }.map(" + c + "::from_handle)", ev="Option<" + c + ">")
    if cat == "Group":
        if name == "Objects":
            return T(rin=None, rout="Vec<Instance>", eout="u32", lout="rt_href(v)", conv="instances(r)", ev="Value")
        if name in ("Array", "Dictionary", "Map"):
            return T(rin=None, rout="Value", eout="u32", lout="rt_href(v)", conv="unsafe { Value::from_raw(r) }", ev="Value")
        if name == "Variant":
            return T(rin="impl IntoValue", ext=[("u32", "x.raw()")], pre="let x = x.into_value();", n=1, lin="rt_hget({0})",
                     rout="Value", eout="u32", lout="rt_href(v)", conv="unsafe { Value::from_raw(r) }", ev="Value")
        return None
    return None

def event_type(t):
    if isinstance(t, dict) and t["Category"] == "Group" and t["Name"] == "Tuple":
        return "crate::signal::Varargs"
    s = spec(t)
    return s.ev if s else "Value"

def allowed(m, write=False):
    tags = set(m.get("Tags") or [])
    if tags & {"Deprecated", "NotScriptable"}:
        return False
    sec = m.get("Security")
    if isinstance(sec, dict):
        sec = sec["Write" if write else "Read"]
    return sec == "None"

# ---- enums --------------------------------------------------------------------------------
eout = ["//! Roblox enums (generated by gen/gen.py from the API dump; do not edit).",
        "//! Each enum is a `u32` newtype with one associated constant per item.", "",
        "use crate::value::{FromValue, IntoValue, Value};", ""]
for name in sorted(enums):
    e = enums[name]
    tn = type_ident(name)
    eout.append(f"/// `Enum.{name}`")
    eout.append("#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]")
    eout.append("#[repr(transparent)]")
    eout.append(f"pub struct {tn}(pub u32);")
    eout.append(f"impl {tn} {{")
    seen = set()
    for it in e["Items"]:
        if "Deprecated" in (it.get("Tags") or []):
            continue
        iname = type_ident(it["Name"])
        if iname[0].isdigit():
            iname = "_" + iname
        if iname in seen or iname in KEYWORDS:
            continue
        seen.add(iname)
        eout.append(f"    pub const {iname}: {tn} = {tn}({it['Value']});")
    eout.append("}")
    eout.append(f"impl IntoValue for {tn} {{ fn into_value(&self) -> Value {{ self.0.into_value() }} }}")
    eout.append(f"impl FromValue for {tn} {{ fn from_value(v: Value) -> Self {{ {tn}(if v.type_name() == \"EnumItem\" {{ v.get(\"Value\").to_f64() as u32 }} else {{ v.to_f64() as u32 }}) }} }}")
    eout.append("")
open(os.path.join(out_dir, "enums.rs"), "w").write("\n".join(eout))

# ---- classes ------------------------------------------------------------------------------
out = ["//! Roblox classes (generated by gen/gen.py from the API dump; do not edit).", "",
       "use alloc::string::String;", "use alloc::vec::Vec;", "use core::ops::Deref;", "",
       "use crate::datatypes::*;", "use crate::enums;", "use crate::rt::{self, luau_fn, Handle, RawStr};",
       "use crate::signal::Signal;", "use crate::value::{IntoValue, Value};", "use crate::Class;", "",
       "fn instances(r: u32) -> Vec<Instance> {",
       "    let t = unsafe { Value::from_raw(r) };",
       "    (1..=t.len()).filter_map(|i| t.index(i).to_instance::<Instance>()).collect()",
       "}", ""]

stats = {"props": 0, "setters": 0, "methods": 0, "events": 0, "skipped": 0}

def gen_member_fns(c):
    lines = []
    used = set()
    cname = c["Name"]
    def add(fn_name, body_lines):
        if fn_name in used:
            stats["skipped"] += 1
            return
        used.add(fn_name)
        lines.extend(body_lines)
    for m in c["Members"]:
        mt, mname = m["MemberType"], m["Name"]
        if mt == "Property":
            if not allowed(m):
                continue
            s = spec(m["ValueType"])
            if s is None or s.rout is None:
                stats["skipped"] += 1
                continue
            getter = ident(mname)
            add(getter, [
                f"    /// `{cname}.{mname}`",
                f"    pub fn {getter}(&self) -> {s.rout} {{",
                f"        luau_fn! {{ fn g(h: u32) -> {s.eout} = {rust_str(f'function(h) local v = rt_hget(h).{mname} return {s.lout} end')}; }}",
                f"        let r = unsafe {{ g(self.handle().id()) }};",
                f"        {s.conv}",
                "    }"])
            stats["props"] += 1
            tags = set(m.get("Tags") or [])
            ss = spec(m["ValueType"], setter=True)
            if "ReadOnly" not in tags and allowed(m, write=True) and ss and getattr(ss, "rin", None):
                params = ", ".join(f"p{i}" for i in range(ss.n))
                val = ss.lin.format(*[f"p{i}" for i in range(ss.n)])
                ext_params = ", ".join(f"x{i}: {t}" for i, (t, _) in enumerate(ss.ext))
                ext_args = ", ".join(e.replace("x", "x", 1) for _, e in ss.ext)
                code = f"function(h, {params}) rt_hget(h).{mname} = {val} end"
                add("set_" + snake(mname), [
                    f"    /// Set `{cname}.{mname}`",
                    f"    pub fn set_{snake(mname)}(&self, x: {ss.rin}) {{",
                    f"        luau_fn! {{ fn s(h: u32, {ext_params}) = {rust_str(code)}; }}",
                    *( [f"        {ss.pre}"] if getattr(ss, "pre", None) else []),
                    f"        unsafe {{ s(self.handle().id(), {ext_args}) }}",
                    "    }"])
                stats["setters"] += 1
        elif mt == "Function":
            if not allowed(m):
                continue
            ret = m["ReturnType"]
            multi = isinstance(ret, list) or (ret["Category"] == "Group" and ret["Name"] == "Tuple")
            if multi:
                rs = VALUE
            else:
                rs = None if (ret["Category"] == "Primitive" and ret["Name"] == "null") else spec(ret)
            ok = True
            rparams, eparams, eargs, pre, lparams, largs = [], [], [], [], [], []
            pnames = set()
            k = 0
            for p in m["Parameters"]:
                ps = spec(p["Type"])
                if ps is None or not getattr(ps, "rin", None):
                    ok = False
                    break
                pn = ident(p["Name"])
                while pn in pnames or pn in ("h", "r", "s", "g"):
                    pn += "_"
                pnames.add(pn)
                rparams.append(f"{pn}: {ps.rin}")
                if getattr(ps, "pre", None):
                    pre.append("        " + ps.pre.replace("x", pn))
                names = [f"p{k + i}" for i in range(ps.n)]
                for i, (t, e) in enumerate(ps.ext):
                    eparams.append(f"x{k + i}: {t}")
                    eargs.append(re.sub(r"\bx\b", pn, e))
                lparams.extend(names)
                largs.append(ps.lin.format(*names))
                k += ps.n
            if not ok:
                stats["skipped"] += 1
                continue
            fname = ident(mname)
            yields = "Yields" in (m.get("Tags") or [])
            lp = ", ".join(["h"] + lparams)
            call = (f"rt_yield(o.{mname}, o{''.join(', ' + a for a in largs)})" if yields
                    else f"o:{mname}({', '.join(largs)})")
            if rs is None:
                code = f"function({lp}) local o = rt_hget(h) {call} end"
                sig_ret, eret, conv = "", "", None
            else:
                if multi:
                    code = f"function({lp}) local o = rt_hget(h) return rt_href(table.pack({call})) end"
                else:
                    code = f"function({lp}) local o = rt_hget(h) local v = {call} return {rs.lout} end"
                sig_ret, eret, conv = f" -> {rs.rout}", f" -> {rs.eout}", rs.conv
            body = [f"    /// `{cname}:{mname}()`" + (" (yields)" if yields else ""),
                    f"    pub fn {fname}(&self{''.join(', ' + p for p in rparams)}){sig_ret} {{",
                    f"        luau_fn! {{ fn f(h: u32{''.join(', ' + p for p in eparams)}){eret} = {rust_str(code)}; }}",
                    *pre]
            call_rs = f"unsafe {{ f(self.handle().id(){''.join(', ' + a for a in eargs)}) }}"
            if conv is None:
                body.append(f"        {call_rs}")
            else:
                body.append(f"        let r = {call_rs};")
                body.append(f"        {conv}")
            body.append("    }")
            add(fname, body)
            stats["methods"] += 1
        elif mt == "Event":
            if not allowed(m):
                continue
            args = ", ".join(event_type(p["Type"]) for p in m["Parameters"])
            tup = f"({args},)" if len(m["Parameters"]) == 1 else f"({args})"
            ename = ident(mname)
            add(ename, [
                f"    /// `{cname}.{mname}` event",
                f"    pub fn {ename}(&self) -> Signal<{tup}> {{",
                f"        luau_fn! {{ fn e(h: u32) -> u32 = {rust_str(f'function(h) return rt_href(rt_hget(h).{mname}) end')}; }}",
                f"        Signal::from_handle(unsafe {{ Handle::from_raw(e(self.handle().id())) }})",
                "    }"])
            stats["events"] += 1
    return lines

for name in sorted(classes):
    c = classes[name]
    tn = type_ident(name)
    sup = c.get("Superclass")
    tags = set(c.get("Tags") or [])
    out.append(f"/// `{name}`" + (f" (inherits `{sup}`)" if sup and sup != "<<<ROOT>>>" else ""))
    out.append("#[derive(Clone, PartialEq)]")
    out.append("#[repr(transparent)]")
    if sup and sup != "<<<ROOT>>>" and sup in classes:
        stn = type_ident(sup)
        out.append(f"pub struct {tn}({stn});")
        out.append(f"impl Deref for {tn} {{ type Target = {stn}; fn deref(&self) -> &{stn} {{ &self.0 }} }}")
        out.append(f"impl Class for {tn} {{ const NAME: &'static str = {rust_str(name)}; fn from_handle(h: Handle) -> Self {{ {tn}({stn}::from_handle(h)) }} fn handle(&self) -> &Handle {{ self.0.handle() }} }}")
    else:
        out.append(f"pub struct {tn}(Handle);")
        out.append(f"impl Class for {tn} {{ const NAME: &'static str = {rust_str(name)}; fn from_handle(h: Handle) -> Self {{ {tn}(h) }} fn handle(&self) -> &Handle {{ &self.0 }} }}")
    out.append(f"impl IntoValue for {tn} {{ fn into_value(&self) -> Value {{ unsafe {{ Value::from_raw(self.handle().clone().into_raw()) }} }} }}")
    out.append(f"impl core::fmt::Debug for {tn} {{ fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {{ write!(f, \"{name}({{}})\", self.handle().to_string_luau()) }} }}")
    out.append(f"impl {tn} {{")
    if "NotCreatable" not in tags and "Service" not in tags and "Deprecated" not in tags:
        out.append(f"    /// `Instance.new(\"{name}\")`")
        out.append(f"    pub fn new() -> Self {{ crate::new_instance::<Self>() }}")
    if "Service" in tags:
        out.append(f"    /// `game:GetService(\"{name}\")`")
        out.append(f"    pub fn get() -> Self {{")
        out.append(f"        luau_fn! {{ fn s() -> u32 = {rust_str(f'function() return rt_href(game:GetService(' + chr(39) + name + chr(39) + ')) end')}; }}")
        out.append(f"        Self::from_handle(unsafe {{ Handle::from_raw(s()) }})")
        out.append("    }")
    out.extend(gen_member_fns(c))
    out.append("}")
    out.append("")

open(os.path.join(out_dir, "classes.rs"), "w").write("\n".join(out))
print(f"{len(classes)} classes, {len(enums)} enums; {stats}")
