//! Breaks infinitely-sized recursive types with `Box`.
//!
//! A dependency graph is built over every generated struct and variant enum,
//! with one edge per *by-value* containment (direct, through `Option`, a fixed
//! array or a type alias; `Vec` and maps already live on the heap). Tarjan's
//! algorithm yields the strongly connected components; every by-value edge
//! that stays inside a component is part of a cycle and gets boxed, which
//! prevents `E0072: recursive type has infinite size`.

use crate::ir::{Model, Ty};
use std::collections::HashMap;

/// Names of the generated types `ty` contains by value.
fn by_value_refs<'a>(ty: &'a Ty, out: &mut Vec<&'a str>) {
    match ty {
        Ty::Named(name) => out.push(name),
        Ty::Alias { target, .. } => by_value_refs(target, out),
        Ty::Option(inner) | Ty::Array(inner, _) => by_value_refs(inner, out),
        Ty::Boxed(_) | Ty::Vec(..) | Ty::FixedVec(..) | Ty::Map(..) => {}
        Ty::Bool | Ty::Int(..) | Ty::F32 | Ty::F64 | Ty::String | Ty::Nbt | Ty::Unit | Ty::Enum { .. } => {}
    }
}

struct Tarjan<'g> {
    edges: &'g [Vec<usize>],
    index: Vec<Option<usize>>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    next: usize,
    component: Vec<usize>,
    components: usize,
}

impl Tarjan<'_> {
    fn visit(&mut self, v: usize) {
        self.index[v] = Some(self.next);
        self.low[v] = self.next;
        self.next += 1;
        self.stack.push(v);
        self.on_stack[v] = true;
        for i in 0..self.edges[v].len() {
            let w = self.edges[v][i];
            match self.index[w] {
                None => {
                    self.visit(w);
                    self.low[v] = self.low[v].min(self.low[w]);
                }
                Some(index) if self.on_stack[w] => self.low[v] = self.low[v].min(index),
                Some(_) => {}
            }
        }
        if Some(self.low[v]) == self.index[v] {
            while let Some(w) = self.stack.pop() {
                self.on_stack[w] = false;
                self.component[w] = self.components;
                if w == v {
                    break;
                }
            }
            self.components += 1;
        }
    }
}

/// Strongly connected component of every node.
fn components(edges: &[Vec<usize>]) -> Vec<usize> {
    let n = edges.len();
    let mut tarjan = Tarjan {
        edges,
        index: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        component: vec![usize::MAX; n],
        components: 0,
    };
    for v in 0..n {
        if tarjan.index[v].is_none() {
            tarjan.visit(v);
        }
    }
    tarjan.component
}

/// Boxes every by-value occurrence of a type for which `cyclic` holds.
fn box_cyclic(ty: &mut Ty, cyclic: &dyn Fn(&str) -> bool) -> usize {
    match ty {
        Ty::Named(name) if cyclic(name) => {
            *ty = Ty::Boxed(Box::new(ty.clone()));
            1
        }
        Ty::Alias { target, .. } => {
            let mut refs = Vec::new();
            by_value_refs(target, &mut refs);
            if refs.into_iter().any(cyclic) {
                *ty = Ty::Boxed(Box::new(ty.clone()));
                1
            } else {
                0
            }
        }
        Ty::Option(inner) | Ty::Array(inner, _) => box_cyclic(inner, cyclic),
        _ => 0,
    }
}

/// Inserts the `Box`es; returns how many fields / branches were boxed.
pub fn break_cycles(model: &mut Model) -> usize {
    let mut ids: HashMap<String, usize> = HashMap::new();
    for name in model.structs.iter().map(|s| &s.name).chain(model.variants.iter().map(|v| &v.name)) {
        let id = ids.len();
        ids.insert(name.clone(), id);
    }

    let mut edges = vec![Vec::new(); ids.len()];
    let mut add = |from: &str, ty: &Ty| {
        let mut refs = Vec::new();
        by_value_refs(ty, &mut refs);
        for target in refs {
            if let Some(&to) = ids.get(target) {
                edges[ids[from]].push(to);
            }
        }
    };
    for def in &model.structs {
        for field in &def.fields {
            add(&def.name, &field.ty);
        }
    }
    for def in &model.variants {
        for branch in &def.branches {
            add(&def.name, &branch.ty);
        }
    }

    let component = components(&edges);
    let mut boxed = 0;
    let (ids, component) = (&ids, &component);
    let same_component = |owner: &str| {
        let owner = component[ids[owner]];
        move |name: &str| ids.get(name).is_some_and(|&id| component[id] == owner)
    };
    for def in &mut model.structs {
        let cyclic = same_component(&def.name);
        for field in &mut def.fields {
            boxed += box_cyclic(&mut field.ty, &cyclic);
        }
    }
    for def in &mut model.variants {
        let cyclic = same_component(&def.name);
        for branch in &mut def.branches {
            boxed += box_cyclic(&mut branch.ty, &cyclic);
        }
    }
    boxed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::*;

    fn field(name: &str, ty: Ty) -> Field {
        Field { name: name.into(), original: name.into(), doc: None, ty, constant: None }
    }

    fn strukt(name: &str, fields: Vec<Field>) -> StructDef {
        StructDef { name: name.into(), source: name.into(), doc: None, fields }
    }

    fn named(name: &str) -> Ty {
        Ty::Named(name.into())
    }

    fn boxed(ty: Ty) -> Ty {
        Ty::Boxed(Box::new(ty))
    }

    #[test]
    fn tarjan_components() {
        // 0 -> 1 -> 2 -> 0, 2 -> 3, 4 -> 4
        let edges = vec![vec![1], vec![2], vec![0, 3], vec![], vec![4]];
        let c = components(&edges);
        assert!(c[0] == c[1] && c[1] == c[2]);
        assert_ne!(c[0], c[3]);
        assert_ne!(c[3], c[4]);
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn boxes_direct_indirect_and_self_recursion() {
        let mut model = Model::default();
        model.structs = vec![
            // A -> B -> A (indirect), through an Option on one side.
            strukt("A", vec![field("b", named("B")), field("leaf", named("Leaf"))]),
            strukt("B", vec![field("a", Ty::Option(Box::new(named("A"))))]),
            // Self-recursive through an alias and a fixed array.
            strukt(
                "Node",
                vec![
                    field("next", Ty::Option(Box::new(Ty::Alias { name: "NodeRef".into(), target: Box::new(named("Node")) }))),
                    field("pair", Ty::Array(Box::new(named("Node")), 2)),
                    // Already on the heap: must stay untouched.
                    field("children", Ty::Vec(Box::new(named("Node")), LenEnc::Var)),
                ],
            ),
            strukt("Leaf", vec![field("x", Ty::Bool)]),
            // Expr <-> Variant enum.
            strukt("Expr", vec![field("op", named("Op"))]),
        ];
        model.variants = vec![VariantDef {
            name: "Op".into(),
            source: "Op".into(),
            doc: None,
            kind: VariantKind::Index(Int::U32, Enc::Var),
            branches: vec![
                Branch { name: "Not".into(), ty: named("Expr"), tag_variant: None },
                Branch { name: "Leaf".into(), ty: named("Leaf"), tag_variant: None },
            ],
        }];

        assert_eq!(break_cycles(&mut model), 6);
        let s = |name: &str| model.structs.iter().find(|s| s.name == name).unwrap();
        assert_eq!(s("A").fields[0].ty, boxed(named("B")));
        assert_eq!(s("A").fields[1].ty, named("Leaf"));
        assert_eq!(s("B").fields[0].ty, Ty::Option(Box::new(boxed(named("A")))));
        assert_eq!(
            s("Node").fields[0].ty,
            Ty::Option(Box::new(boxed(Ty::Alias { name: "NodeRef".into(), target: Box::new(named("Node")) })))
        );
        assert_eq!(s("Node").fields[1].ty, Ty::Array(Box::new(boxed(named("Node"))), 2));
        assert_eq!(s("Node").fields[2].ty, Ty::Vec(Box::new(named("Node")), LenEnc::Var));
        assert_eq!(s("Expr").fields[0].ty, boxed(named("Op")));
        assert_eq!(model.variants[0].branches[0].ty, boxed(named("Expr")));
        assert_eq!(model.variants[0].branches[1].ty, named("Leaf"));

        // Idempotent: nothing left to box.
        assert_eq!(break_cycles(&mut model), 0);
    }
}
