//! Canonical textual and binary forms for the object graph.
//!
//! Requirements this satisfies, all of them structural rather than by
//! convention:
//!
//! * **Lossless round trip.** Print then parse yields a graph with the same
//!   shape, including sharing and cycles.
//! * **Alpha-invariant binders.** Binders are stored as de Bruijn indices and
//!   carry no variable identity at all — the printer *generates* `v0`, `v1` on
//!   the way out. Two alpha-equivalent expressions therefore print identically
//!   and hash identically, which is what a content-addressed store requires.
//!   (This used to claim the opposite — "variables are objects, so a printed
//!   form carries their identity" — describing the design that was replaced
//!   precisely because a name in an unhashed field gave two alpha-equivalent
//!   nodes one id and two encodings.)
//! * **Cycles.** Datum labels — `#3=(...)` to define, `#3#` to refer — so a
//!   self-referential proposition prints and reparses.
//! * **Unknown-node preservation.** An `Opaque` node from a producer this build
//!   does not understand survives the trip byte-for-byte.
//! * **Versioned binary.** A format byte leads every encoding.
//!
//! The surface syntax serializes the universal graph, not a smaller convenience
//! grammar — anything representable prints, including operators with no
//! semantics.

use crate::object::{
    Binder, Binding, CoreNode, ExternalRef, LiteralValue, ObjectGraph, ObjectId, wk,
};
use num_bigint::BigInt;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Binary format version. Bump on any encoding change.
///
/// 2: stored binder slots lost their variable id. A slot is now a domain and
/// nothing else, so a v1 encoding would be misread field-for-field rather than
/// failing loudly.
pub const BINARY_VERSION: u8 = 2;

// ---- printing ----------------------------------------------------------

/// Print `root` in canonical S-expression form.
pub fn print(g: &ObjectGraph, root: ObjectId) -> String {
    // Anything reached more than once, or reached cyclically, needs a label.
    let mut seen = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    mark(g, root, &mut seen, &mut repeated, &mut Vec::new());

    let mut labels: BTreeMap<ObjectId, usize> = BTreeMap::new();
    for (i, id) in repeated.iter().enumerate() {
        labels.insert(*id, i + 1);
    }
    let mut emitted = BTreeSet::new();
    let mut out = String::new();
    let mut scope: Vec<String> = Vec::new();
    write_node(g, root, &labels, &mut emitted, &mut scope, &mut out);
    out
}

fn mark(
    g: &ObjectGraph,
    id: ObjectId,
    seen: &mut BTreeSet<ObjectId>,
    repeated: &mut BTreeSet<ObjectId>,
    path: &mut Vec<ObjectId>,
) {
    if path.contains(&id) {
        repeated.insert(id);
        return;
    }
    if !seen.insert(id) {
        // Only compound nodes are worth labelling; atoms print compactly.
        if g.as_bvar(id).is_none()
            && matches!(
                g.get(id),
                Some(CoreNode::Apply { .. }) | Some(CoreNode::Bind { .. })
            )
        {
            repeated.insert(id);
        }
        return;
    }
    path.push(id);
    for c in g.children(id) {
        mark(g, c, seen, repeated, path);
    }
    path.pop();
}

fn write_node(
    g: &ObjectGraph,
    id: ObjectId,
    labels: &BTreeMap<ObjectId, usize>,
    emitted: &mut BTreeSet<ObjectId>,
    scope: &mut Vec<String>,
    out: &mut String,
) {
    if let Some(n) = labels.get(&id) {
        if emitted.contains(&id) {
            let _ = write!(out, "#{n}#");
            return;
        }
        emitted.insert(id);
        let _ = write!(out, "#{n}=");
    }

    match g.get(id) {
        None => {
            let _ = write!(out, "{id}");
        }
        Some(CoreNode::Atom { name }) => match name {
            // The truth atoms print with the sigil the reader expects, so they
            // survive a round trip as themselves rather than as Bool literals.
            _ if id == wk::TOP => out.push_str("#true"),
            _ if id == wk::BOT => out.push_str("#false"),
            Some(n) => out.push_str(&escape_atom(n)),
            None => {
                let _ = write!(out, "{id}");
            }
        },
        Some(CoreNode::Literal(v)) => out.push_str(&print_literal(v)),
        Some(CoreNode::Apply { operator, operands }) => {
            // A bound occurrence prints as the name its binder was given, not
            // as `(bvar k)`: the index is the storage form, the name is the
            // reading form, and both denote the same slot.
            if let Some(k) = g.as_bvar(id) {
                match scope.len().checked_sub(k + 1).and_then(|i| scope.get(i)) {
                    Some(name) => out.push_str(name),
                    // Free of every enclosing binder — print the index, which
                    // is the honest thing to say about an unbound occurrence.
                    None => {
                        let _ = write!(out, "(bvar {k})");
                    }
                }
                return;
            }
            out.push('(');
            write_node(g, *operator, labels, emitted, scope, out);
            for o in operands {
                out.push(' ');
                write_node(g, *o, labels, emitted, scope, out);
            }
            out.push(')');
        }
        Some(CoreNode::Bind {
            binder,
            vars,
            bodies,
        }) => {
            out.push('(');
            write_node(g, *binder, labels, emitted, scope, out);
            out.push_str(" [");
            // Slots carry no name, so the canonical printer generates one.
            // Display names are deliberately not stored: a name inside the node
            // that the hash ignored would give alpha-equivalent expressions one
            // id and two different byte encodings, and `intern`'s
            // first-writer-wins would pick between them by accident.
            let outer = scope.len();
            for (i, b) in vars.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                let name = format!("v{}", outer + i);
                let _ = write!(out, "({name}");
                if let Some(d) = b.domain {
                    out.push(' ');
                    // Telescoping: slot i's domain sees slots 0..i.
                    scope.truncate(outer);
                    for j in 0..i {
                        scope.push(format!("v{}", outer + j));
                    }
                    write_node(g, d, labels, emitted, scope, out);
                }
                out.push(')');
            }
            out.push(']');
            scope.truncate(outer);
            for i in 0..vars.len() {
                scope.push(format!("v{}", outer + i));
            }
            for b in bodies {
                out.push(' ');
                write_node(g, *b, labels, emitted, scope, out);
            }
            scope.truncate(outer);
            out.push(')');
        }
        Some(CoreNode::External(r)) => {
            let _ = write!(
                out,
                "(#external {} {}",
                escape_atom(&r.namespace),
                hex_field(&r.locator)
            );
            match &r.version {
                Some(v) => {
                    let _ = write!(out, " {}", hex_field(v));
                }
                None => out.push_str(" -"),
            }
            match &r.digest {
                Some(d) => {
                    let _ = write!(out, " {}", hex_field(d));
                }
                None => out.push_str(" -"),
            }
            out.push(')');
        }
        Some(CoreNode::Opaque { tag, payload }) => {
            let _ = write!(out, "(#opaque {} {})", escape_atom(tag), hex_field(payload));
        }
    }
}

fn print_literal(v: &LiteralValue) -> String {
    match v {
        LiteralValue::Int(n) => n.to_string(),
        LiteralValue::Decimal { mantissa, scale } => format!("{mantissa}d{scale}"),
        LiteralValue::Text(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
        LiteralValue::Bool(b) => b.to_string(),
        LiteralValue::Bytes(b) => format!("0x{}", hex(b)),
    }
}

fn escape_atom(s: &str) -> String {
    if s.is_empty()
        || s.chars().any(|c| c.is_whitespace() || "()\"#|".contains(c))
        || reads_as_literal(s)
    {
        format!("|{}|", s.replace('|', "\\|"))
    } else {
        s.to_string()
    }
}

/// Would this bare token be read back as a **literal** rather than an atom?
///
/// The guard used to be `s.parse::<BigInt>().is_ok()` alone, which covers
/// exactly one of the four literal syntaxes the reader recognises. So an atom
/// named `1d2` printed bare and reparsed as `Decimal { mantissa: 1, scale: 2 }`,
/// `0xff` as `Bytes([255])`, and `true` as a `Bool` — the round-trip property
/// held on the *text* every time while the meaning changed underneath it.
///
/// That is the same bug as `wk::TOP` printing as `true`, in the section of the
/// spec that describes that bug as fixed. Text equality was too weak to catch
/// it then and is too weak now, which is why `tests/properties.rs` compares
/// *evaluated results* over generated names.
///
/// This mirrors `Reader::atom_or_literal` and has to keep mirroring it: a new
/// literal syntax without a matching case here reintroduces the whole class.
fn reads_as_literal(s: &str) -> bool {
    if s == "true" || s == "false" {
        return true;
    }
    if let Some(h) = s.strip_prefix("0x") {
        return unhex(h).is_some();
    }
    if let Some((m, scale)) = s.split_once('d')
        && m.parse::<BigInt>().is_ok()
        && scale.parse::<i32>().is_ok()
    {
        return true;
    }
    s.parse::<BigInt>().is_ok()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A byte field, where **empty is a value and needs a glyph**.
///
/// `hex(&[])` is zero characters wide, so an `Opaque` with an empty payload
/// printed as `(#opaque e )` and the reader failed on the missing token. Empty
/// also cannot borrow `-`, which already means *absent* for the optional
/// version and digest — `Some(vec![])` and `None` are different facts.
fn hex_field(b: &[u8]) -> String {
    if b.is_empty() {
        ".".to_string()
    } else {
        hex(b)
    }
}

/// Inverse of [`hex_field`]; `None` here means the field was malformed, not
/// that it was absent — absence is the caller's `-` check.
fn unhex_field(s: &str) -> Option<Vec<u8>> {
    if s == "." {
        return Some(Vec::new());
    }
    unhex(s)
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

// ---- parsing -----------------------------------------------------------

pub fn parse(g: &mut ObjectGraph, src: &str) -> Result<ObjectId, String> {
    let mut p = Parser {
        s: src.as_bytes(),
        i: 0,
        labels: BTreeMap::new(),
    };
    let id = p.expr(g)?;
    p.ws();
    if p.i < p.s.len() {
        return Err(format!("trailing input at byte {}", p.i));
    }
    Ok(id)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    labels: BTreeMap<usize, ObjectId>,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    /// Consume `kw` when it appears as a whole head token.
    ///
    /// The delimiter check is what makes this safe: without it `#opaque` would
    /// also match a hypothetical `#opaquer`, and a head that merely *starts*
    /// with the keyword would be silently rewritten into a node shape.
    fn eat_head(&mut self, kw: &[u8]) -> bool {
        if !self.s[self.i..].starts_with(kw) {
            return false;
        }
        match self.s.get(self.i + kw.len()) {
            // End of input ends the token too: `#true` may be the whole
            // expression, not only a head inside a list.
            None => {
                self.i += kw.len();
                true
            }
            Some(c) if (*c as char).is_whitespace() || *c == b'(' || *c == b')' || *c == b'[' => {
                self.i += kw.len();
                true
            }
            Some(_) => false,
        }
    }

    fn expr(&mut self, g: &mut ObjectGraph) -> Result<ObjectId, String> {
        self.ws();
        // Datum label: #N=... defines, #N# refers, #<hex> is a raw id.
        if self.peek() == Some(b'#') {
            // The truth *atoms*, before the raw-id reader gets to them: `#true`
            // is not hex and would otherwise fail as "bad object id".
            if self.eat_head(b"#true") {
                return Ok(wk::TOP);
            }
            if self.eat_head(b"#false") {
                return Ok(wk::BOT);
            }
            let start = self.i;
            self.i += 1;
            let digits = self.take_while(|c| c.is_ascii_digit());
            if !digits.is_empty() && self.peek() == Some(b'#') {
                self.i += 1;
                let n: usize = digits.parse().map_err(|_| "bad label")?;
                return self
                    .labels
                    .get(&n)
                    .copied()
                    .ok_or_else(|| format!("forward label #{n}#"));
            }
            if !digits.is_empty() && self.peek() == Some(b'=') {
                self.i += 1;
                let n: usize = digits.parse().map_err(|_| "bad label")?;
                // Reserve before parsing the body so a cycle can close.
                let id = g.alloc();
                self.labels.insert(n, id);
                let body = self.expr(g)?;
                let node = g.get(body).cloned().ok_or("empty labelled body")?;
                g.define(id, node);
                return Ok(id);
            }
            // Raw id.
            self.i = start + 1;
            let h = self.take_while(|c| c.is_ascii_hexdigit());
            let raw = u128::from_str_radix(&h, 16).map_err(|_| "bad object id")?;
            return Ok(ObjectId(raw));
        }

        if self.peek() == Some(b'(') {
            self.i += 1;
            self.ws();
            // `External` and `Opaque` are node *shapes*, not applications, so
            // they need a head the reader can tell from an operator. They used
            // to be recognised by the atom names `external` and `opaque`, which
            // silently reserved those two names: `(external a)` as an ordinary
            // application failed to parse at all. The `#` sigil cannot collide
            // with a raw id or a datum label, both of which require digits
            // immediately after it.
            if self.eat_head(b"#external") {
                return self.finish_external(g);
            }
            if self.eat_head(b"#opaque") {
                return self.finish_opaque(g);
            }
            let head = self.expr(g)?;
            // Binder form: second token is a parenthesised binding list.
            self.ws();
            let is_bind = self.peek() == Some(b'[');
            if is_bind {
                self.i += 1; // consume '['
                let mut vars = Vec::new();
                loop {
                    self.ws();
                    match self.peek() {
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        Some(b'(') => {
                            self.i += 1;
                            let var = self.expr(g)?;
                            self.ws();
                            let domain = if self.peek() == Some(b')') {
                                None
                            } else {
                                Some(self.expr(g)?)
                            };
                            self.ws();
                            if self.peek() != Some(b')') {
                                return Err("unterminated binding".into());
                            }
                            self.i += 1;
                            vars.push(Binding { var, domain });
                        }
                        _ => return Err("malformed binding list".into()),
                    }
                }
                let mut bodies = Vec::new();
                loop {
                    self.ws();
                    if self.peek() == Some(b')') {
                        self.i += 1;
                        break;
                    }
                    bodies.push(self.expr(g)?);
                }
                return Ok(g.bind(head, vars, bodies));
            }
            let mut operands = Vec::new();
            loop {
                self.ws();
                match self.peek() {
                    Some(b')') => {
                        self.i += 1;
                        break;
                    }
                    None => return Err("unterminated list".into()),
                    _ => operands.push(self.expr(g)?),
                }
            }
            return Ok(g.apply(head, operands));
        }

        self.atom_or_literal(g)
    }

    fn finish_external(&mut self, g: &mut ObjectGraph) -> Result<ObjectId, String> {
        let ns = self.token()?;
        let loc = self.token()?;
        let ver = self.token()?;
        let dig = self.token()?;
        self.ws();
        if self.peek() != Some(b')') {
            return Err("unterminated external".into());
        }
        self.i += 1;
        let digest = if dig == "-" {
            None
        } else {
            let v = unhex_field(&dig).ok_or("bad digest")?;
            let mut d = [0u8; 32];
            if v.len() != 32 {
                return Err("digest must be 32 bytes".into());
            }
            d.copy_from_slice(&v);
            Some(d)
        };
        Ok(g.external(ExternalRef {
            namespace: unquote(&ns),
            locator: unhex_field(&loc).ok_or("bad locator")?,
            version: if ver == "-" {
                None
            } else {
                Some(unhex_field(&ver).ok_or("bad version")?)
            },
            digest,
        }))
    }

    fn finish_opaque(&mut self, g: &mut ObjectGraph) -> Result<ObjectId, String> {
        let tag = self.token()?;
        let payload = self.token()?;
        self.ws();
        if self.peek() != Some(b')') {
            return Err("unterminated opaque".into());
        }
        self.i += 1;
        Ok(g.intern(CoreNode::Opaque {
            tag: unquote(&tag),
            payload: unhex_field(&payload).ok_or("bad opaque payload")?,
        }))
    }

    fn token(&mut self) -> Result<String, String> {
        self.ws();
        if self.peek() == Some(b'|') {
            self.i += 1;
            let mut out = String::new();
            while let Some(c) = self.peek() {
                self.i += 1;
                if c == b'\\' {
                    if let Some(n) = self.peek() {
                        self.i += 1;
                        out.push(n as char);
                    }
                    continue;
                }
                if c == b'|' {
                    return Ok(format!("|{out}|"));
                }
                out.push(c as char);
            }
            return Err("unterminated |atom|".into());
        }
        let t = self.take_while(|c| !c.is_whitespace() && c != '(' && c != ')');
        if t.is_empty() {
            Err("expected token".into())
        } else {
            Ok(t)
        }
    }

    fn take_while(&mut self, f: impl Fn(char) -> bool) -> String {
        let start = self.i;
        while self.i < self.s.len() && f(self.s[self.i] as char) {
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    fn atom_or_literal(&mut self, g: &mut ObjectGraph) -> Result<ObjectId, String> {
        self.ws();
        if self.peek() == Some(b'"') {
            self.i += 1;
            let mut out = String::new();
            while let Some(c) = self.peek() {
                self.i += 1;
                match c {
                    b'\\' => {
                        if let Some(n) = self.peek() {
                            self.i += 1;
                            out.push(n as char);
                        }
                    }
                    b'"' => return Ok(g.lit(LiteralValue::Text(out))),
                    _ => out.push(c as char),
                }
            }
            return Err("unterminated string".into());
        }
        let t = self.token()?;
        // `true`/`false` are Bool *literals*; the well-known truth *atoms* are
        // `#true`/`#false` and are handled in the `#` branch above, before the
        // raw-id reader. They are different objects with different evaluation
        // behaviour — the atoms are Supported/Refuted, the literals are
        // Unsupported — and the atoms previously had no surface form at all, so
        // a graph containing `wk::TOP` printed as "true", reparsed as a
        // literal, and silently changed meaning while satisfying the
        // round-trip property.
        if t == "true" {
            return Ok(g.boolean(true));
        }
        if t == "false" {
            return Ok(g.boolean(false));
        }
        // A malformed byte literal is an *atom named* `0x…`, not an error. The
        // reader used to bail here, which made `0x.f` unreadable even though
        // nothing could have produced it as a literal — so the printer and the
        // reader disagreed about which tokens were literals at all, and the
        // disagreement was a parse failure rather than a wrong meaning. The
        // decimal case has always fallen through this way.
        if let Some(h) = t.strip_prefix("0x")
            && let Some(bytes) = unhex(h)
        {
            return Ok(g.lit(LiteralValue::Bytes(bytes)));
        }
        if let Some((m, s)) = t.split_once('d')
            && let (Ok(mantissa), Ok(scale)) = (m.parse::<BigInt>(), s.parse::<i32>())
        {
            return Ok(g.lit(LiteralValue::Decimal { mantissa, scale }));
        }
        if let Ok(n) = t.parse::<BigInt>() {
            return Ok(g.lit(LiteralValue::Int(n)));
        }
        Ok(g.atom(&unquote(&t)))
    }
}

fn unquote(s: &str) -> String {
    s.strip_prefix('|')
        .and_then(|x| x.strip_suffix('|'))
        .map(|x| x.to_string())
        .unwrap_or_else(|| s.to_string())
}

// ---- binary ------------------------------------------------------------

/// Encode the subgraph reachable from `root`, version-tagged.
pub fn to_bytes(g: &ObjectGraph, root: ObjectId) -> Vec<u8> {
    let mut out = vec![BINARY_VERSION];
    let ids: Vec<ObjectId> = g.reachable(root).into_iter().collect();
    push_u64(&mut out, ids.len() as u64);
    push_u128(&mut out, root.0);
    for id in &ids {
        push_u128(&mut out, id.0);
        match g.get(*id) {
            None => out.push(0),
            Some(CoreNode::Atom { name }) => {
                out.push(1);
                push_str(&mut out, name.as_deref().unwrap_or(""));
                out.push(name.is_some() as u8);
            }
            Some(CoreNode::Literal(v)) => {
                out.push(2);
                encode_literal(v, &mut out);
            }
            Some(CoreNode::Apply { operator, operands }) => {
                out.push(3);
                push_u128(&mut out, operator.0);
                push_u64(&mut out, operands.len() as u64);
                for o in operands {
                    push_u128(&mut out, o.0);
                }
            }
            Some(CoreNode::Bind {
                binder,
                vars,
                bodies,
            }) => {
                out.push(4);
                push_u128(&mut out, binder.0);
                push_u64(&mut out, vars.len() as u64);
                // A stored slot is a domain and nothing else — no variable id,
                // which is what makes alpha-equivalent binders one object.
                for b in vars {
                    match b.domain {
                        Some(d) => {
                            out.push(1);
                            push_u128(&mut out, d.0);
                        }
                        None => out.push(0),
                    }
                }
                push_u64(&mut out, bodies.len() as u64);
                for b in bodies {
                    push_u128(&mut out, b.0);
                }
            }
            Some(CoreNode::External(r)) => {
                out.push(5);
                push_str(&mut out, &r.namespace);
                push_bytes(&mut out, &r.locator);
                match &r.version {
                    Some(v) => {
                        out.push(1);
                        push_bytes(&mut out, v);
                    }
                    None => out.push(0),
                }
                match &r.digest {
                    Some(d) => {
                        out.push(1);
                        out.extend_from_slice(d);
                    }
                    None => out.push(0),
                }
            }
            Some(CoreNode::Opaque { tag, payload }) => {
                out.push(6);
                push_str(&mut out, tag);
                push_bytes(&mut out, payload);
            }
        }
    }
    out
}

/// Decode into `g`, returning the root. Ids are preserved exactly.
pub fn from_bytes(g: &mut ObjectGraph, buf: &[u8]) -> Result<ObjectId, String> {
    let mut c = Cursor { b: buf, i: 0 };
    let v = c.u8()?;
    if v != BINARY_VERSION {
        return Err(format!("unsupported binary version {v}"));
    }
    let n = c.u64()? as usize;
    let root = ObjectId(c.u128()?);
    for _ in 0..n {
        let id = ObjectId(c.u128()?);
        let tag = c.u8()?;
        let node = match tag {
            0 => continue,
            1 => {
                let name = c.string()?;
                let present = c.u8()? == 1;
                CoreNode::Atom {
                    name: present.then_some(name),
                }
            }
            2 => CoreNode::Literal(decode_literal(&mut c)?),
            3 => {
                let operator = ObjectId(c.u128()?);
                let k = c.u64()? as usize;
                let mut operands = Vec::with_capacity(k);
                for _ in 0..k {
                    operands.push(ObjectId(c.u128()?));
                }
                CoreNode::Apply { operator, operands }
            }
            4 => {
                let binder = ObjectId(c.u128()?);
                let k = c.u64()? as usize;
                let mut vars = Vec::with_capacity(k);
                for _ in 0..k {
                    let domain = if c.u8()? == 1 {
                        Some(ObjectId(c.u128()?))
                    } else {
                        None
                    };
                    vars.push(Binder { domain });
                }
                let m = c.u64()? as usize;
                let mut bodies = Vec::with_capacity(m);
                for _ in 0..m {
                    bodies.push(ObjectId(c.u128()?));
                }
                CoreNode::Bind {
                    binder,
                    vars,
                    bodies,
                }
            }
            5 => {
                let namespace = c.string()?;
                let locator = c.bytes()?;
                let version = if c.u8()? == 1 { Some(c.bytes()?) } else { None };
                let digest = if c.u8()? == 1 {
                    let mut d = [0u8; 32];
                    d.copy_from_slice(c.take(32)?);
                    Some(d)
                } else {
                    None
                };
                CoreNode::External(ExternalRef {
                    namespace,
                    locator,
                    version,
                    digest,
                })
            }
            6 => CoreNode::Opaque {
                tag: c.string()?,
                payload: c.bytes()?,
            },
            other => return Err(format!("unknown node tag {other}")),
        };
        g.define(id, node);
    }
    Ok(root)
}

fn encode_literal(v: &LiteralValue, out: &mut Vec<u8>) {
    match v {
        LiteralValue::Int(n) => {
            out.push(0);
            push_bytes(out, &n.to_signed_bytes_le());
        }
        LiteralValue::Decimal { mantissa, scale } => {
            out.push(1);
            push_bytes(out, &mantissa.to_signed_bytes_le());
            out.extend_from_slice(&scale.to_le_bytes());
        }
        LiteralValue::Text(s) => {
            out.push(2);
            push_str(out, s);
        }
        LiteralValue::Bool(b) => {
            out.push(3);
            out.push(*b as u8);
        }
        LiteralValue::Bytes(b) => {
            out.push(4);
            push_bytes(out, b);
        }
    }
}

fn decode_literal(c: &mut Cursor<'_>) -> Result<LiteralValue, String> {
    Ok(match c.u8()? {
        0 => LiteralValue::Int(BigInt::from_signed_bytes_le(&c.bytes()?)),
        1 => {
            let mantissa = BigInt::from_signed_bytes_le(&c.bytes()?);
            let mut s = [0u8; 4];
            s.copy_from_slice(c.take(4)?);
            LiteralValue::Decimal {
                mantissa,
                scale: i32::from_le_bytes(s),
            }
        }
        2 => LiteralValue::Text(c.string()?),
        3 => LiteralValue::Bool(c.u8()? == 1),
        4 => LiteralValue::Bytes(c.bytes()?),
        other => return Err(format!("unknown literal tag {other}")),
    })
}

fn push_u64(o: &mut Vec<u8>, v: u64) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn push_u128(o: &mut Vec<u8>, v: u128) {
    o.extend_from_slice(&v.to_le_bytes());
}
fn push_bytes(o: &mut Vec<u8>, b: &[u8]) {
    push_u64(o, b.len() as u64);
    o.extend_from_slice(b);
}
fn push_str(o: &mut Vec<u8>, s: &str) {
    push_bytes(o, s.as_bytes());
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.i + n > self.b.len() {
            return Err("unexpected end of input".into());
        }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u64(&mut self) -> Result<u64, String> {
        let mut a = [0u8; 8];
        a.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(a))
    }
    fn u128(&mut self) -> Result<u128, String> {
        let mut a = [0u8; 16];
        a.copy_from_slice(self.take(16)?);
        Ok(u128::from_le_bytes(a))
    }
    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.u64()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn string(&mut self) -> Result<String, String> {
        String::from_utf8(self.bytes()?).map_err(|e| e.to_string())
    }
}
