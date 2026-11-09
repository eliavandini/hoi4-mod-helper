//! Paradox's binary asset format (`.mesh`, `.anim`): the `@@b@` header, then
//! a tree of objects (`[` repeated once per depth, a name ending in a zero
//! byte) holding properties (`!`, a one-byte name length, the name, a type
//! `i` / `f` / `s`, a count, the values; each string a length that counts
//! its ending zero byte, then the bytes).

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq)]
pub enum Prop {
    Ints(Vec<i32>),
    Floats(Vec<f32>),
    Strs(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub name: String,
    pub props: Vec<(String, Prop)>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn prop(&self, k: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.0 == k).map(|p| &p.1)
    }
    pub fn floats(&self, k: &str) -> &[f32] {
        match self.prop(k) {
            Some(Prop::Floats(v)) => v,
            _ => &[],
        }
    }
    pub fn ints(&self, k: &str) -> &[i32] {
        match self.prop(k) {
            Some(Prop::Ints(v)) => v,
            _ => &[],
        }
    }
    pub fn str(&self, k: &str) -> Option<&str> {
        match self.prop(k) {
            Some(Prop::Strs(v)) => v.first().map(String::as_str),
            _ => None,
        }
    }
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        if self.at + n > self.b.len() {
            bail!("the file ends inside a value (byte {})", self.at);
        }
        let s = &self.b[self.at..self.at + n];
        self.at += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
}

/// Parse a whole file: the root node holds the top-level objects and the
/// properties before them (`pdxasset`).
pub fn parse(b: &[u8]) -> Result<Node> {
    if !b.starts_with(b"@@b@") {
        if b.starts_with(b"@@t@") || b.first().is_some_and(|c| c.is_ascii_alphabetic()) {
            bail!("a text asset, not a binary one (only binary .mesh / .anim files are read)");
        }
        bail!("not a Paradox binary asset (no @@b@ header)");
    }
    let mut r = Reader { b, at: 4 };
    // the path from the root to the open object
    let mut stack: Vec<Node> = vec![Node::default()];
    while r.at < b.len() {
        match b[r.at] {
            b'!' => {
                r.at += 1;
                let n = r.take(1)?[0] as usize;
                let name = String::from_utf8_lossy(r.take(n)?).into_owned();
                let kind = r.take(1)?[0];
                let count = r.u32()? as usize;
                if count > b.len() {
                    bail!("property {name} claims {count} values");
                }
                let prop = match kind {
                    b'i' => Prop::Ints(r.take(count * 4)?.chunks_exact(4).map(|c| i32::from_le_bytes(c.try_into().unwrap())).collect()),
                    b'f' => Prop::Floats(r.take(count * 4)?.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()),
                    b's' => {
                        let mut v = Vec::with_capacity(count);
                        for _ in 0..count {
                            let len = r.u32()? as usize;
                            let s = r.take(len)?;
                            let s = s.strip_suffix(&[0]).unwrap_or(s);
                            v.push(String::from_utf8_lossy(s).into_owned());
                        }
                        Prop::Strs(v)
                    }
                    k => bail!("property {name} has an unknown type {:?} (byte {})", k as char, r.at - 5),
                };
                stack.last_mut().unwrap().props.push((name, prop));
            }
            b'[' => {
                let mut depth = 0;
                while r.at < b.len() && b[r.at] == b'[' {
                    depth += 1;
                    r.at += 1;
                }
                let end = b[r.at..].iter().position(|&c| c == 0).ok_or_else(|| anyhow::anyhow!("an object name never ends"))?;
                let name = String::from_utf8_lossy(&b[r.at..r.at + end]).into_owned();
                r.at += end + 1;
                if depth > stack.len() {
                    bail!("object {name} is {depth} deep under an object {} deep", stack.len() - 1);
                }
                // close the objects at this depth or deeper
                while stack.len() > depth {
                    let done = stack.pop().unwrap();
                    stack.last_mut().unwrap().children.push(done);
                }
                stack.push(Node { name, ..Default::default() });
            }
            c => bail!("unexpected byte {c:#04x} at {}", r.at),
        }
    }
    while stack.len() > 1 {
        let done = stack.pop().unwrap();
        stack.last_mut().unwrap().children.push(done);
    }
    Ok(stack.pop().unwrap())
}

/// Write a tree in the same format (generated models, tests).
pub fn write(root: &Node) -> Vec<u8> {
    fn props(out: &mut Vec<u8>, n: &Node) {
        for (k, p) in &n.props {
            out.push(b'!');
            out.push(k.len() as u8);
            out.extend_from_slice(k.as_bytes());
            match p {
                Prop::Ints(v) => {
                    out.push(b'i');
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
                }
                Prop::Floats(v) => {
                    out.push(b'f');
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
                }
                Prop::Strs(v) => {
                    out.push(b's');
                    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
                    for s in v {
                        out.extend_from_slice(&(s.len() as u32 + 1).to_le_bytes());
                        out.extend_from_slice(s.as_bytes());
                        out.push(0);
                    }
                }
            }
        }
    }
    fn node(out: &mut Vec<u8>, n: &Node, depth: usize) {
        out.extend(std::iter::repeat_n(b'[', depth));
        out.extend_from_slice(n.name.as_bytes());
        out.push(0);
        props(out, n);
        for c in &n.children {
            node(out, c, depth + 1);
        }
    }
    let mut out = b"@@b@".to_vec();
    props(&mut out, root);
    for c in &root.children {
        node(&mut out, c, 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let leaf = Node { name: "aabb".into(), props: vec![("min".into(), Prop::Floats(vec![-1.0, 0.0, -1.0]))], children: vec![] };
        let mesh = Node {
            name: "mesh".into(),
            props: vec![("tri".into(), Prop::Ints(vec![0, 1, 2])), ("p".into(), Prop::Floats(vec![0.0; 9]))],
            children: vec![leaf, Node { name: "material".into(), props: vec![("shader".into(), Prop::Strs(vec!["PdxMeshStandard".into()]))], children: vec![] }],
        };
        let root = Node {
            name: String::new(),
            props: vec![("pdxasset".into(), Prop::Ints(vec![1, 0]))],
            children: vec![
                Node { name: "object".into(), props: vec![], children: vec![Node { name: "shape".into(), props: vec![], children: vec![mesh] }] },
                Node { name: "locator".into(), props: vec![], children: vec![] },
            ],
        };
        let b = write(&root);
        assert_eq!(parse(&b).unwrap(), root);
        assert!(parse(b"@@t@ hello").is_err());
        assert!(parse(&b[..b.len() - 3]).is_err(), "a name cut short");
        assert!(parse(&b[..30]).is_err(), "a value cut short");
    }
}
