//! Which script modules an app uses, learned by running it (a collect run,
//! a film's analysis). The web optimiser then drops the registration of
//! every module that was registered and not used, and with it the code and
//! the embedded Splash only that registration reached.
//!
//! While recording, each `script_mod` (the function `script_mod!` makes)
//! is bracketed with [`ScriptVm::census_begin`] / [`ScriptVm::census_end`]
//! under its `module_path!()`: the values it sets in the module scope
//! (`mod.widgets.Button = ...`) are its tops, and every object made while
//! it ran is its own. At the end ([`census_used`]) the heap is walked from
//! everything no module made (the app's objects, widget sources, the
//! objects Rust holds): a module's top reached marks the module used, and
//! then all that module made is walked too, until nothing is added. A name
//! merely bound (`use mod.widgets.*`, a prelude's spread) is not a use; the
//! module scopes and the preludes are walls.

use crate::heap::*;
use crate::tokenizer::ScriptToken;
use crate::makepad_live_id::*;
use crate::value::*;
use crate::vm::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// What the module registrations made, while recording.
#[derive(Default)]
pub struct Census {
    modules: Vec<&'static str>,
    tops: HashMap<ScriptObject, (usize, LiveId)>,
    made: HashMap<ScriptObject, usize>,
    /// What Rust held when a registration ended (a type's proto, a
    /// registry's template): holding it then is not a use.
    held_at_registration: HashSet<ScriptObject>,
    /// Modules that made a module scope of their own (`mod.sdf`): named as
    /// a whole (`..mod.sdf`, `mod.ime.X`), so always used.
    defines: HashMap<usize, LiveId>,
    /// The script bodies each registration evaluated (by body index).
    bodies: HashMap<usize, usize>,
    stack: Vec<Bracket>,
}

struct Bracket {
    module: usize,
    scopes: HashSet<LiveId>,
    bodies: usize,
    values: HashMap<(ScriptObject, LiveId), ScriptValue>,
    objects: HashSet<ScriptObject>,
}

/// The modules an app registered, and those it used with what used each
/// first (`the app's script`, or the using module's path).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModuleUse {
    pub registered: BTreeSet<String>,
    pub used: BTreeMap<String, String>,
}

fn module_values(heap: &ScriptHeap) -> HashMap<(ScriptObject, LiveId), ScriptValue> {
    let mut out = HashMap::new();
    for (_, m) in heap.objects[heap.modules].map.iter() {
        let Some(module) = m.value.as_object() else { continue };
        for (key, entry) in heap.objects[module].map.iter() {
            if let Some(key) = key.as_id() {
                out.insert((module, key), entry.value);
            }
        }
    }
    out
}

fn module_scopes(heap: &ScriptHeap) -> HashSet<LiveId> {
    heap.objects[heap.modules].map.iter().filter_map(|(k, _)| k.as_id()).collect()
}

fn alloced(heap: &ScriptHeap) -> Vec<ScriptObject> {
    (1..heap.objects.len())
        .filter(|&i| heap.objects.get_at(i).tag.is_alloced())
        .map(|i| ScriptObject::new(i as u32, heap.objects.generation(i)))
        .collect()
}

fn valid(heap: &ScriptHeap, obj: ScriptObject) -> bool {
    heap.objects.is_valid(obj) && heap.objects[obj].tag.is_alloced()
}

/// The walk: from `seeds`, through protos, map and vec entries and arrays,
/// asking `follow(from, key, to)` before each object.
fn reach(
    heap: &ScriptHeap,
    seeds: &[ScriptObject],
    visited: &mut HashSet<ScriptObject>,
    follow: &mut dyn FnMut(ScriptObject, ScriptValue, ScriptObject) -> bool,
) {
    let mut stack: Vec<ScriptObject> = seeds.iter().copied().filter(|s| valid(heap, *s) && visited.insert(*s)).collect();
    let mut arrays: HashSet<ScriptArray> = HashSet::new();
    let mut edges: Vec<(ScriptValue, ScriptValue)> = Vec::new();
    while let Some(from) = stack.pop() {
        let object = &heap.objects[from];
        edges.clear();
        edges.push((NIL, object.proto));
        edges.extend(object.map.iter().map(|(k, e)| (*k, e.value)));
        edges.extend(object.vec.iter().map(|kv| (kv.key, kv.value)));
        let mut i = 0;
        while i < edges.len() {
            let (key, value) = edges[i];
            i += 1;
            if let Some(to) = value.as_object() {
                if valid(heap, to) && !visited.contains(&to) && follow(from, key, to) {
                    visited.insert(to);
                    stack.push(to);
                }
            } else if let Some(arr) = value.as_array() {
                if heap.arrays.is_valid(arr) && arrays.insert(arr) {
                    for j in 0..heap.array_len(arr) {
                        edges.push((NIL, heap.array_index_unchecked(arr, j)));
                    }
                }
            }
        }
    }
}

impl ScriptVm<'_> {
    /// Records the module registrations from now on.
    pub fn census_record(&mut self) {
        if self.bx.heap.census.is_none() {
            self.bx.heap.census = Some(Box::default());
        }
    }

    /// Before a module's `script_mod` registers (a no-op unless recording).
    pub fn census_begin(&mut self, module_path: &'static str) {
        if self.bx.heap.census.is_none() {
            return;
        }
        let values = module_values(&self.bx.heap);
        let scopes = module_scopes(&self.bx.heap);
        let bodies = self.bx.code.bodies.borrow().len();
        let objects = alloced(&self.bx.heap).into_iter().collect();
        let census = self.bx.heap.census.as_mut().unwrap();
        let module = match census.modules.iter().position(|m| *m == module_path) {
            Some(i) => i,
            None => {
                census.modules.push(module_path);
                census.modules.len() - 1
            }
        };
        census.stack.push(Bracket { module, scopes, bodies, values, objects });
    }

    /// After it: what it set and made is its own (an inner module's keeps
    /// what it claimed first).
    pub fn census_end(&mut self) {
        let Some(bracket) = self.bx.heap.census.as_mut().and_then(|c| c.stack.pop()) else { return };
        let after = module_values(&self.bx.heap);
        let made: Vec<ScriptObject> = alloced(&self.bx.heap).into_iter().filter(|o| !bracket.objects.contains(o)).collect();
        let held: Vec<ScriptObject> = self.bx.heap.root_objects.borrow().keys().copied().collect();
        let scopes = module_scopes(&self.bx.heap);
        let bodies = self.bx.code.bodies.borrow().len();
        let census = self.bx.heap.census.as_mut().unwrap();
        for body in bracket.bodies..bodies {
            census.bodies.entry(body).or_insert(bracket.module);
        }
        if let Some(scope) = scopes.difference(&bracket.scopes).next() {
            census.defines.entry(bracket.module).or_insert(*scope);
        }
        census.held_at_registration.extend(held);
        for ((module, key), value) in after {
            let Some(obj) = value.as_object() else { continue };
            if bracket.values.get(&(module, key)) == Some(&value) || census.tops.contains_key(&obj) {
                continue;
            }
            census.tops.insert(obj, (bracket.module, key));
        }
        for obj in made {
            census.made.entry(obj).or_insert(bracket.module);
        }
    }
}

/// The modules registered and used (see the module docs); `None` when the
/// census was not recording. Runs a garbage collection first.
pub fn census_used(vm: &mut ScriptVm) -> Option<ModuleUse> {
    vm.bx.heap.census.as_ref()?;
    vm.gc();
    let census = vm.bx.heap.census.take()?;
    let heap = &vm.bx.heap;
    let name = |m: usize| census.modules[m].to_string();
    let mut out = ModuleUse { registered: census.modules.iter().map(|m| m.to_string()).collect(), used: BTreeMap::new() };
    let tops: HashMap<ScriptObject, (usize, LiveId)> = census.tops.iter().filter(|(o, _)| valid(heap, **o)).map(|(o, v)| (*o, *v)).collect();
    // The module scopes and the preludes hold every name: walls.
    let mut walls: HashSet<ScriptObject> = HashSet::new();
    walls.insert(heap.modules);
    for (key, m) in heap.objects[heap.modules].map.iter() {
        let Some(module) = m.value.as_object() else { continue };
        // A module scope is made with its name as its proto; anything else
        // set at the top (`mod.theme = mod.themes.dark`) is an ordinary
        // value, walked from the start.
        if heap.objects[module].proto != *key {
            continue;
        }
        walls.insert(module);
        if key.as_id() == Some(id!(prelude)) {
            walls.extend(heap.objects[module].map.iter().filter_map(|(_, v)| v.value.as_object()));
        }
    }
    // What the modules made, and all their values reach.
    let mut owned: HashMap<usize, Vec<ScriptObject>> = HashMap::new();
    for (obj, (m, _)) in &tops {
        owned.entry(*m).or_default().push(*obj);
    }
    for (obj, m) in &census.made {
        if valid(heap, *obj) {
            owned.entry(*m).or_default().push(*obj);
        }
    }
    let seeds: Vec<ScriptObject> = owned.values().flatten().copied().collect();
    let mut module_made: HashSet<ScriptObject> = HashSet::new();
    reach(heap, &seeds, &mut module_made, &mut |_, _, to| !walls.contains(&to));
    // The walk starts at everything else, and at the tops Rust holds.
    let mut seeds: Vec<ScriptObject> = alloced(heap).into_iter().filter(|o| !module_made.contains(o) && !walls.contains(o)).collect();
    let mut label: HashMap<ScriptObject, usize> = HashMap::new();
    let mut used: HashMap<usize, String> = HashMap::new();
    // Rust holding a module's object (the app's root widget's source, a
    // template looked up by name) uses the module.
    for obj in heap.root_objects.borrow().keys() {
        if census.held_at_registration.contains(obj) {
            continue;
        }
        let Some(m) = tops.get(obj).map(|(m, _)| *m).or_else(|| census.made.get(obj).copied()) else { continue };
        if valid(heap, *obj) && !used.contains_key(&m) {
            used.insert(m, "held by Rust".into());
            for o in owned.get(&m).into_iter().flatten() {
                label.insert(*o, m);
                seeds.push(*o);
            }
        }
    }
    for (m, scope) in &census.defines {
        if !used.contains_key(m) {
            used.insert(*m, format!("defines mod.{scope}"));
            for o in owned.get(m).into_iter().flatten() {
                label.insert(*o, *m);
                seeds.push(*o);
            }
        }
    }
    // Names: a script body naming another module's top uses it, whether
    // or not the line ran yet (a shader resolves its helpers by name when
    // it compiles, a function when it is called).
    let mut top_names: HashMap<LiveId, Vec<usize>> = HashMap::new();
    for (m, top) in tops.values() {
        top_names.entry(*top).or_default().push(*m);
    }
    let bodies = vm.bx.code.bodies.borrow();
    let named_by = |body: usize, out: &mut Vec<(usize, LiveId)>| {
        let (dot, spread) = (LiveId::from_str("."), LiveId::from_str(".."));
        if let Some(b) = bodies.get(body) {
            let tokens = &b.tokenizer.tokens;
            for (i, t) in tokens.iter().enumerate() {
                let ScriptToken::Identifier(id) = t.token else { continue };
                let Some(modules) = top_names.get(&id) else { continue };
                // Made (`X{`), called (`X(`), reached into (`X.f`) or
                // spread (`..X`); a bare name is a value (`place: Right`),
                // too often another module's word for something else.
                let next = tokens.get(i + 1).map(|t| &t.token);
                let prev = i.checked_sub(1).map(|p| &tokens[p].token);
                let named = matches!(next, Some(ScriptToken::OpenCurly | ScriptToken::OpenRound))
                    || matches!(next, Some(ScriptToken::Operator(op)) if *op == dot)
                    || matches!(prev, Some(ScriptToken::Operator(op)) if *op == spread);
                if named {
                    out.extend(modules.iter().map(|m| (*m, id)));
                }
            }
        }
    };
    let mut names: Vec<(usize, LiveId)> = Vec::new();
    for body in 0..bodies.len() {
        if !census.bodies.contains_key(&body) {
            named_by(body, &mut names);
        }
    }
    let by_name = |names: &mut Vec<(usize, LiveId)>, from: Option<usize>, used: &mut HashMap<usize, String>, fresh: &mut Vec<usize>| {
        for (m, id) in names.drain(..) {
            if !used.contains_key(&m) {
                used.insert(m, format!("{} names {id}", from.map_or_else(|| "the app's script".to_string(), name)));
                fresh.push(m);
            }
        }
    };
    let mut visited: HashSet<ScriptObject> = HashSet::new();
    let mut fresh: Vec<usize> = Vec::new();
    by_name(&mut names, None, &mut used, &mut fresh);
    for m in fresh.drain(..) {
        for o in owned.get(&m).into_iter().flatten() {
            label.insert(*o, m);
            seeds.push(*o);
        }
    }
    let mut module_bodies: HashMap<usize, Vec<usize>> = HashMap::new();
    for (body, m) in &census.bodies {
        module_bodies.entry(*m).or_default().push(*body);
    }
    let mut named_done: HashSet<usize> = HashSet::new();
    loop {
        let mut fresh: Vec<usize> = Vec::new();
        reach(heap, &seeds, &mut visited, &mut |from, key, to| {
            if walls.contains(&to) {
                return false;
            }
            let from_label = label.get(&from).copied();
            if let Some((m, top_name)) = tops.get(&to) {
                // A name bound is not a use.
                if key.as_id() == Some(*top_name) {
                    return false;
                }
                if !used.contains_key(m) {
                    used.insert(*m, from_label.map_or_else(|| "the app's script".to_string(), name));
                    fresh.push(*m);
                }
                label.insert(to, *m);
            } else if let Some(l) = from_label {
                label.insert(to, l);
            }
            true
        });
        // The names in every used module's own script.
        let in_use: Vec<usize> = used.keys().copied().filter(|m| !named_done.contains(m)).collect();
        for m in in_use {
            named_done.insert(m);
            for body in module_bodies.get(&m).into_iter().flatten() {
                named_by(*body, &mut names);
            }
            by_name(&mut names, Some(m), &mut used, &mut fresh);
        }
        if fresh.is_empty() {
            break;
        }
        seeds = Vec::new();
        for m in fresh {
            for o in owned.get(&m).into_iter().flatten() {
                label.insert(*o, m);
                seeds.push(*o);
            }
        }
    }
    out.used = used.into_iter().map(|(m, by)| (name(m), by)).collect();
    Some(out)
}
