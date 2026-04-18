use lopdf::Document;

fn main() -> anyhow::Result<()> {
    let bytes = std::fs::read("../amonedthorr1.pdf").unwrap();
    let doc = Document::load_mem(&bytes)?;
    let pages = doc.get_pages();
    println!("{} Pages", pages.len());
    for (npage, oid) in doc.page_iter().enumerate() {
        println!("#Page {npage}");
        if let Ok(cont) = doc.get_page_annotations(oid) {
            for ann in cont.iter() {
                for (_, obj) in ann.iter() {
                    dbg!(obj);
                }
            }
        }
    }
    //let out = pdf_extract::extract_text_from_mem(&bytes).unwrap();
    //dbg!(out);
    Ok(())
}
