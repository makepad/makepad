use crate::structure::{StructElement, StructKid, StructTree};
use crate::{ObjRef, PdfDocument, PdfObj};

// Hand-authored PDF 1.7 objects. This fixture deliberately shares no writer code.
fn fixture() -> Vec<String> {
    let mut objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R /StructTreeRoot 8 0 R /MarkInfo << /Marked true >> /Lang (de-DE) /ViewerPreferences << /Direction /L2R >> >>", // 1
        "<< /Type /Pages /Kids [4 0 R 5 0 R] /Count 2 /MediaBox [0 0 612 792] >>", // 2
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>", // 3
        "<< /Type /Page /Parent 2 0 R /Contents 6 0 R /StructParents 0 /Resources << /Font << /F1 3 0 R >> /Properties << /MC1 << /MCID 1 >> >> >> /Annots [22 0 R] >>", // 4
        "<< /Type /Page /Parent 2 0 R /Contents 7 0 R /StructParents 1 /Resources << /Font << /F1 3 0 R >> >> >>", // 5
        "", // 6: first page stream
        "", // 7: second page stream
        "<< /Type /StructTreeRoot /K 12 0 R /RoleMap << /Absatz /P >> /ParentTree 9 0 R /ParentTreeNextKey 3 >>", // 8
        "<< /Kids [10 0 R 11 0 R] >>", // 9
        "<< /Limits [0 0] /Nums [0 [13 0 R 14 0 R 17 0 R 18 0 R 19 0 R 20 0 R 21 0 R 23 0 R 24 0 R]] >>", // 10
        "<< /Limits [1 2] /Nums [1 [14 0 R 28 0 R 31 0 R] 2 21 0 R] >>", // 11
        "<< /Type /StructElem /S /Document /P 8 0 R /K [13 0 R 14 0 R 15 0 R 19 0 R 20 0 R 21 0 R 23 0 R 24 0 R 25 0 R] >>", // 12
        "<< /Type /StructElem /S /H1 /P 12 0 R /Pg 4 0 R /K 0 >>", // 13
        "<< /Type /StructElem /S /Absatz /P 12 0 R /Pg 4 0 R /K [1 << /Type /MCR /Pg 5 0 R /MCID 0 >>] /A [<< /O /Layout /Placement /Block /WritingMode /LrTb >> 0] >>", // 14
        "<< /Type /StructElem /S /L /P 12 0 R /K 16 0 R /A << /O /List /ListNumbering /Decimal >> >>", // 15
        "<< /Type /StructElem /S /LI /P 15 0 R /K [17 0 R 18 0 R] >>", // 16
        "<< /Type /StructElem /S /Lbl /P 16 0 R /Pg 4 0 R /K 2 >>", // 17
        "<< /Type /StructElem /S /LBody /P 16 0 R /Pg 4 0 R /K << /Type /MCR /MCID 3 >> >>", // 18
        "<< /Type /StructElem /S /P /P 12 0 R /Pg 4 0 R /K 4 >>", // 19
        "<< /Type /StructElem /S /P /P 12 0 R /Pg 4 0 R /K 5 >>", // 20
        "<< /Type /StructElem /S /Link /P 12 0 R /Pg 4 0 R /K [6 << /Type /OBJR /Obj 22 0 R >>] >>", // 21
        "<< /Type /Annot /Subtype /Link /P 4 0 R /Rect [40 400 100 420] /StructParent 2 /Dest [5 0 R /Fit] >>", // 22
        "<< /Type /StructElem /S /Figure /P 12 0 R /Pg 4 0 R /K 7 /Alt <FEFF00420069006C0064> >>", // 23
        "<< /Type /StructElem /S /P /P 12 0 R /Pg 4 0 R /K 8 /ActualText (Ersatz) /Lang (de-AT) /E (Expansion) /T (Titel des Absatzes) /ID (p-id) >>", // 24
        "<< /Type /StructElem /S /Table /P 12 0 R /K [26 0 R 29 0 R] >>", // 25
        "<< /Type /StructElem /S /THead /P 25 0 R /K 27 0 R >>", // 26
        "<< /Type /StructElem /S /TR /P 26 0 R /K 28 0 R >>", // 27
        "<< /Type /StructElem /S /TH /P 27 0 R /Pg 5 0 R /K 1 /A << /O /Table /Scope /Column >> >>", // 28
        "<< /Type /StructElem /S /TBody /P 25 0 R /K 30 0 R >>", // 29
        "<< /Type /StructElem /S /TR /P 29 0 R /K 31 0 R >>", // 30
        "<< /Type /StructElem /S /TD /P 30 0 R /Pg 5 0 R /K 2 >>", // 31
    ].into_iter().map(str::to_owned).collect();
    objects[5] = stream(concat!(
        "BT /F1 12 Tf 1 0 0 1 40 740 Tm\n",
        "/H1 << /MCID 0 >> BDC (Titel) Tj EMC\n",
        "/P /MC1 BDC (Eins ) Tj EMC\n",
        "/Lbl << /MCID 2 >> BDC (1.) Tj EMC\n",
        "/LBody << /MCID 3 >> BDC (Punkt) Tj EMC\n",
        "/P << /MCID 4 >> BDC\n",
        "/Span << /ActualText (fi) >> BDC (x) Tj EMC\n",
        "/ReversedChars BMC (cba) Tj EMC EMC\n",
        "/P << /MCID 5 >> BDC 1 0 0 1 40 500 Tm\n",
        "(Zeile eins) Tj 0 -14 Td (Zeile zwei) Tj EMC\n",
        "/Link << /MCID 6 >> BDC (Link) Tj EMC\n",
        "/P << /MCID 8 >> BDC (Original) Tj EMC\n",
        "/Artifact << /Type /Pagination /Subtype /Footer >> BDC\n",
        "1 0 0 1 40 20 Tm (Seite 1) Tj EMC ET\n",
        "/Figure << /MCID 7 >> BDC 40 300 30 30 re f EMC\n",
        "/Artifact BMC 40 10 30 4 re f EMC\n",
    ));
    objects[6] = stream(concat!(
        "BT /F1 12 Tf 1 0 0 1 40 740 Tm\n",
        "/P << /MCID 0 >> BDC (Zwei) Tj EMC\n",
        "/TH << /MCID 1 >> BDC (Spalte) Tj EMC\n",
        "/TD << /MCID 2 >> BDC (Wert) Tj EMC\n",
        "/Artifact << /Type /Pagination /Subtype /Footer >> BDC\n",
        "1 0 0 1 40 20 Tm (Seite 1) Tj EMC ET\n",
    ));
    objects
}

fn stream(content: &str) -> String {
    format!("<< /Length {} >>\nstream\n{}\nendstream", content.len(), content)
}

fn assemble(objects: &[String]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.7\n".to_vec();
    let mut offsets = vec![0];
    for (index, body) in objects.iter().enumerate() {
        // A mutation of stream text must also update its explicit byte length.
        let body = if let Some((_, content)) = body.split_once("\nstream\n") {
            stream(content.strip_suffix("\nendstream").unwrap())
        } else {
            body.clone()
        };
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in offsets.iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", offsets.len(),
    ).as_bytes());
    pdf
}

fn read(objects: &[String]) -> StructTree {
    let bytes = assemble(objects);
    let mut doc = PdfDocument::parse(&bytes).unwrap();
    StructTree::read(&mut doc).unwrap().unwrap()
}

fn clean(tree: &StructTree) {
    assert!(tree.problems.is_empty(), "problems: {:#?}", tree.problems);
    assert!(tree.notes.is_empty(), "notes: {:#?}", tree.notes);
}

fn shape(element: &StructElement) -> String {
    let children: Vec<_> = element.elements().map(shape).collect();
    let kind = if element.kind == element.standard {
        element.kind.clone()
    } else {
        format!("{}={}", element.kind, element.standard)
    };
    if children.is_empty() { kind } else { format!("{kind}({})", children.join(",")) }
}

#[test]
fn reads_hand_written_tagged_structure() {
    let objects = fixture();
    let bytes = assemble(&objects);
    let mut doc = PdfDocument::parse(&bytes).unwrap();
    assert_eq!(doc.page_object_number(0), Some(4));
    assert_eq!(doc.page_object_number(1), Some(5));
    assert_eq!(doc.page_object_number(2), None);
    assert_eq!(doc.page_index_of(5), Some(1));
    assert_eq!(doc.page_index_of(22), None);
    assert_eq!(doc.catalog().unwrap().get_name("Type"), Some("Catalog"));
    let page = doc.page(0).unwrap();
    assert_eq!(page.struct_parents, Some(0));
    assert_eq!(page.properties["MC1"].get_int("MCID"), Some(1));

    let tree = StructTree::read(&mut doc).unwrap().unwrap();
    clean(&tree);
    assert!(tree.marked);
    assert_eq!(tree.lang.as_deref(), Some("de-DE"));
    assert_eq!(tree.direction.as_deref(), Some("L2R"));
    assert_eq!(tree.roots.len(), 1);
    assert_eq!(shape(&tree.roots[0]), "Document(H1,Absatz=P,L(LI(Lbl,LBody)),P,P,Link,Figure,P,Table(THead(TR(TH)),TBody(TR(TD))))");
    assert_eq!(tree.artifacts, vec![vec!["Seite 1", ""], vec!["Seite 1"]]);
    assert_eq!(tree.untagged, vec![Vec::<String>::new(), Vec::new()]);
    let root = &tree.roots[0];
    assert_eq!(root.object, Some(ObjRef { num: 12, gen: 0 }));
    assert_eq!(root.page, None);
    let children: Vec<_> = root.elements().collect();
    let expected = ["Titel", "Eins Zwei", "1.Punkt", "fiabc", "Zeile eins\nZeile zwei", "Link", "", "Ersatz", "SpalteWert"];
    for (element, expected) in children.iter().zip(expected) {
        assert_eq!(element.text(), expected, "text for {}", shape(element));
    }
    assert_eq!(root.text(), "TitelEins Zwei1.PunktfiabcZeile eins\nZeile zweiLinkErsatzSpalteWert");
    assert_eq!(children[0].page, Some(0));
    assert!(matches!(&children[0].kids[..], [StructKid::Content { page: 0, mcid: 0, text }] if text == "Titel"));
    assert!(matches!(&children[1].kids[..], [StructKid::Content { page: 0, mcid: 1, text: first }, StructKid::Content { page: 1, mcid: 0, text: second }] if first == "Eins " && second == "Zwei"));
    assert_eq!(children[1].attribute("Layout", "WritingMode"), Some(&PdfObj::Name("LrTb".into())));
    assert_eq!(children[1].attribute("Layout", "Placement"), Some(&PdfObj::Name("Block".into())));
    assert_eq!(children[1].attribute("List", "WritingMode"), None);
    assert_eq!(children[2].attribute("List", "ListNumbering"), Some(&PdfObj::Name("Decimal".into())));
    let item = children[2].elements().next().unwrap();
    let list_leaves: Vec<_> = item.elements().collect();
    assert_eq!(list_leaves[0].text(), "1.");
    assert_eq!(list_leaves[1].text(), "Punkt");
    assert!(matches!(&children[5].kids[..], [StructKid::Content { page: 0, mcid: 6, text }, StructKid::Object { page: Some(0), object: ObjRef { num: 22, gen: 0 } }] if text == "Link"));
    assert_eq!(children[6].alt.as_deref(), Some("Bild"));
    assert_eq!(children[7].actual_text.as_deref(), Some("Ersatz"));
    assert_eq!(children[7].lang.as_deref(), Some("de-AT"));
    assert_eq!(children[7].expansion.as_deref(), Some("Expansion"));
    assert_eq!(children[7].title.as_deref(), Some("Titel des Absatzes"));
    assert_eq!(children[7].id.as_deref(), Some("p-id"));
    assert!(matches!(&children[7].kids[..], [StructKid::Content { page: 0, mcid: 8, text }] if text == "Original"));
    let groups: Vec<_> = children[8].elements().collect();
    let th = groups[0].elements().next().unwrap().elements().next().unwrap();
    let td = groups[1].elements().next().unwrap().elements().next().unwrap();
    assert_eq!(th.text(), "Spalte");
    assert_eq!(td.text(), "Wert");
    assert_eq!(th.attribute("Table", "Scope"), Some(&PdfObj::Name("Column".into())));
}

#[test]
fn diagnoses_mutated_tagged_structure() {
    let source = fixture();
    let baseline = read(&source);
    clean(&baseline);
    let expected_text = baseline.roots[0].text();
    // Object number, old source, replacement source, required section prefix.
    let mutations = [
        (13, "/P 12 0 R", "/P 15 0 R", "14.7.2"),
        (10, "[13 0 R 14 0 R", "[14 0 R 14 0 R", "14.7.4.4"),
        (6, "(Titel) Tj EMC", "(Titel) Tj EMC /H1 << /MCID 0 >> BDC (doppelt) Tj EMC", "14.7.4.2"),
        (6, "(Titel) Tj EMC", "(Titel) Tj EMC (frei) Tj", "14.8.2.2"),
        (6, "/H1 << /MCID 0 >> BDC (Titel) Tj EMC", "/Artifact BMC /H1 << /MCID 0 >> BDC (Titel) Tj EMC EMC", "14.8.2.2.2"),
        (6, "(Titel) Tj EMC", "(Titel) Tj EMC EMC", "14.6"),
        (8, "/Absatz /P", "", "14.8.4"),
        (14, "/WritingMode /LrTb", "/WritingMode /Sideways", "14.8.5"),
        (1, "/Marked true", "/Marked false", "14.7.1"),
        (13, "/Pg 4 0 R", "", "14.7.4.2"),
        (22, "/StructParent 2", "", "14.7.4.4"),
        (4, "/StructParents 0", "", "14.7.4.4"),
        (8, "/Absatz /P", "/Absatz /Loop /Loop /Absatz", "14.8.4"),
        (12, "/K [13 0 R", "/K [12 0 R 13 0 R", "14.7.2"),
        (6, "(Titel) Tj EMC", "(Titel) Tj EMC /Unclosed BMC", "14.6"),
        (6, "(Titel) Tj EMC", "(Titel) Tj EMC /Shade sh", "14.8.2.2"),
        (1, "/MarkInfo << /Marked true >>", "", "14.7.1"),
        (8, "/Type /StructTreeRoot", "/Type /StructElem", "14.7.2"),
        (13, "/S /H1", "", "14.7.2"),
        (13, "/P 12 0 R", "", "14.7.2"),
        (13, "/Type /StructElem", "/Type /Other", "14.7.2"),
        (12, "/K [13 0 R", "/K [13 0 R 13 0 R", "14.7.2"),
        (18, "/MCID 3", "", "14.7.4.2"),
        (18, "/Pg 4 0 R", "", "14.7.4.2"),
        (6, "/H1 << /MCID 0 >> BDC", "/H1 BMC", "14.7.4.2"),
        (6, "(Titel) Tj EMC", "(Titel) Tj EMC /Span << /MCID 99 >> BDC (verwaist) Tj EMC", "14.7.4.2"),
        (13, "/K 0", "/K [0 0]", "14.7.4.2"),
        (10, "/Nums [0 ", "/Nums [3 ", "14.7.4.4"),
        (11, "2 21 0 R", "2 23 0 R", "14.7.4.4"),
        (14, "/O /Layout", "", "14.7.5"),
        (14, "/Placement /Block", "/Placement /Floating", "14.8.5"),
        (15, "/ListNumbering /Decimal", "/ListNumbering /Arabic", "14.8.5"),
        (28, "/Scope /Column", "/Scope /Diagonal", "14.8.5"),
        (10, "/Limits [0 0]", "/Limits [0 1]", "7.9.7"),
    ];
    for (object, old, new, section) in mutations {
        let mut changed = source.clone();
        assert!(changed[object - 1].contains(old), "mutation source in object {object}");
        changed[object - 1] = changed[object - 1].replacen(old, new, 1);
        let tree = read(&changed);
        assert!(tree.problems.iter().any(|problem| problem.starts_with(section)),
            "object {object}: {old:?} -> {new:?}, expected {section}; problems: {:#?}; notes: {:#?}", tree.problems, tree.notes);
    }

    // Equivalent legal spellings must not inherit a particular writer's choices.
    let alternatives = [
        (8, "/K 12 0 R", "/K [12 0 R]"),
        (13, "/Type /StructElem", ""),
        (13, "/K 0", "/K [0]"),
        (13, "/Pg 4 0 R /K 0", "/K << /Type /MCR /Pg 4 0 R /MCID 0 >>"),
        (21, "/Type /OBJR /Obj", "/Type /OBJR /Pg 4 0 R /Obj"),
        (8, "/Absatz /P", "/Absatz /Custom /Custom /P"),
        (14, "/A [<< /O /Layout /Placement /Block /WritingMode /LrTb >> 0]", "/A << /O /Layout /Placement /Block /WritingMode /LrTb >>"),
        (14, "/A [<< /O /Layout /Placement /Block /WritingMode /LrTb >> 0]", "/A [<< /O /Layout /Placement /Block >> << /O /Layout /WritingMode /LrTb >>]"),
        (5, "/Resources << /Font", "/Resources << /Properties null /Font"),
        (4, "/Properties << /MC1", "/Properties << /Unused null /MC1"),
        (6, "/P /MC1 BDC", "/P << /MCID 1 >> BDC"),
        (6, "/Span << /ActualText (fi) >> BDC (x) Tj EMC", "/Span << /ActualText (fi) >> BDC EMC"),
        (6, "/Span << /ActualText (fi) >> BDC (x) Tj EMC", "/Span << /ActualText <FEFF00660069> >> BDC /Span << /ActualText (inner) >> BDC (x) Tj EMC (y) Tj EMC"),
        (6, "(Zeile eins) Tj 0 -14 Td (Zeile zwei) Tj", "(Zeile eins) Tj 14 TL (Zeile zwei) '"),
        (6, "(Zeile eins) Tj 0 -14 Td (Zeile zwei) Tj", "(Zeile eins) Tj 14 TL 0 0 (Zeile zwei) \""),
        (6, "(Zeile eins) Tj 0 -14 Td (Zeile zwei) Tj", "(Zeile eins) Tj 14 TL T* (Zeile zwei) Tj"),
        (6, "(Zeile eins) Tj 0 -14 Td (Zeile zwei) Tj", "(Zeile eins) Tj 0 -14 TD (Zeile zwei) Tj"),
        (6, "40 10 30 4 re f EMC", "40 10 30 4 re f EMC 0 0 m 1 1 l n"),
        (6, "/Artifact BMC 40 10 30 4 re f EMC", "/Artifact BMC /Artifact BMC 40 10 30 4 re f EMC EMC"),
        (9, "<< /Kids [10 0 R 11 0 R] >>", "<< /Nums [0 [13 0 R 14 0 R 17 0 R 18 0 R 19 0 R 20 0 R 21 0 R 23 0 R 24 0 R] 1 [14 0 R 28 0 R 31 0 R] 2 21 0 R] >>"),
    ];
    for (object, old, new) in alternatives {
        let mut changed = source.clone();
        assert!(changed[object - 1].contains(old));
        changed[object - 1] = changed[object - 1].replacen(old, new, 1);
        let tree = read(&changed);
        clean(&tree);
        assert_eq!(tree.roots[0].text(), expected_text, "equivalent form {new}");
        assert_eq!(tree.artifacts, baseline.artifacts, "artifacts for {new}");
        assert_eq!(tree.untagged, baseline.untagged, "untagged text for {new}");
        let paragraph = tree.roots[0].elements().nth(1).unwrap();
        assert_eq!(paragraph.attribute("Layout", "Placement"), Some(&PdfObj::Name("Block".into())));
        assert_eq!(paragraph.attribute("Layout", "WritingMode"), Some(&PdfObj::Name("LrTb".into())));
    }

    let mut changed = source.clone();
    changed[3] = changed[3].replace("/Properties << /MC1 << /MCID 1 >> >>", "/Properties 32 0 R");
    changed.push("<< /MC1 33 0 R >>".into());
    changed.push("<< /MCID 34 0 R >>".into());
    changed.push("1".into());
    let tree = read(&changed);
    clean(&tree);
    assert_eq!(tree.roots[0].text(), expected_text);

    // /K itself and an MCR dictionary may both be indirect objects.
    for kid in ["[0]", "<< /Type /MCR /MCID 0 >>"] {
        let mut changed = source.clone();
        changed[12] = changed[12].replace("/K 0", "/K 32 0 R");
        changed.push(kid.into());
        let tree = read(&changed);
        clean(&tree);
        assert_eq!(tree.roots[0].text(), expected_text, "indirect kid {kid}");
    }

    // Recommended list/table child types produce notes, not required-rule failures.
    for (object, old, new, section) in [
        (15, "/S /L", "/S /LI", "14.8.4.3.3"),
        (28, "/S /TH", "/S /P", "14.8.4.3.4"),
    ] {
        let mut changed = source.clone();
        assert!(changed[object - 1].contains(old));
        changed[object - 1] = changed[object - 1].replacen(old, new, 1);
        let tree = read(&changed);
        assert!(tree.problems.is_empty(), "problems: {:#?}", tree.problems);
        assert!(tree.notes.iter().any(|note| note.starts_with(section)), "notes: {:#?}", tree.notes);
    }

    let mut changed = source.clone();
    changed[5] = changed[5].replace("(cba) Tj", "[(cb) 20 (a)] TJ");
    let tree = read(&changed);
    clean(&tree);
    assert_eq!(tree.roots[0].elements().nth(3).unwrap().text(), "fibca", "reverse each TJ string, retaining string order");

    let mut changed = source.clone();
    changed[5] = changed[5].replace("(Titel) Tj EMC", "(Titel) Tj EMC (frei) Tj");
    let tree = read(&changed);
    assert_eq!(tree.untagged, vec![vec!["frei"], Vec::<&str>::new()]);
    assert!(tree.problems.iter().any(|problem| problem.starts_with("14.8.2.2") && problem.contains("Tj") && problem.contains("frei")));

    let mut changed = source;
    changed[0] = changed[0].replace("/StructTreeRoot 8 0 R", "");
    let bytes = assemble(&changed);
    let mut doc = PdfDocument::parse(&bytes).unwrap();
    assert!(StructTree::read(&mut doc).unwrap().is_none());
}
