use lopdf::Document;

fn main() -> anyhow::Result<()> {
    let bytes = std::fs::read("form0.pdf").unwrap();
    let doc = Document::load_mem(&bytes)?;
    let pages = doc.get_pages();
    println!("{} Pages", pages.len());
    for (npage, oid) in doc.page_iter().enumerate() {
        println!("#Page {npage}");
        if let Ok(cont) = doc.get_page_annotations(oid) {
            dbg!(cont);
            // for ann in cont.iter() {
            //     for (nm, obj) in ann.iter() {
            //         if let Ok(s) = str::from_utf8(nm) {
            //             dbg!((s, obj));
            //         } else {
            //             dbg!((nm, obj));
            //         }
            //     }
            // }
        }
    }
    //let out = pdf_extract::extract_text_from_mem(&bytes).unwrap();
    //dbg!(out);
    Ok(())
}
