use osmpbf::{Element, ElementReader};

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let needle = std::env::args().nth(2).unwrap();
    ElementReader::from_path(&path)
        .unwrap()
        .for_each(|element| {
            let (kind, id, tags): (&str, i64, Vec<(String, String)>) = match &element {
                Element::Way(w) => ("way", w.id(), w.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
                Element::Relation(r) => ("rel", r.id(), r.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
                Element::Node(n) => ("node", n.id(), n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
                Element::DenseNode(n) => ("node", n.id(), n.tags().map(|(k, v)| (k.to_string(), v.to_string())).collect()),
            };
            if tags.iter().any(|(k, v)| k == "name" && v == &needle) {
                println!("{} {} {:?}", kind, id, tags);
            }
        })
        .unwrap();
}
