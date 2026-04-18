use anyhow::{anyhow, Context, Result};
use lopdf::{Document, Object};
use std::env;
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let mut show_bbox = false;
    let mut path = None;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bbox" | "-b" => show_bbox = true,
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => return Err(anyhow!("Usage: form_fields [--bbox] <pdf-path>")),
        }
    }

    let path = path.context("Usage: form_fields [--bbox] <pdf-path>")?;
    let doc = Document::load(&path)?;
    let catalog = doc.catalog()?;

    let acro_form = match catalog.get(b"AcroForm").ok() {
        Some(obj) => doc.dereference(obj)?.1,
        None => {
            println!("No AcroForm dictionary found in {}", path.display());
            return Ok(());
        }
    };

    let acro_dict = acro_form.as_dict()?;
    let fields = match acro_dict.get(b"Fields").ok() {
        Some(obj) => doc.dereference(obj)?.1.as_array()?,
        None => {
            println!("No form fields found in {}", path.display());
            return Ok(());
        }
    };

    if fields.is_empty() {
        println!("No form fields found in {}", path.display());
        return Ok(());
    }

    println!("Form fields for {}:\n", path.display());
    for field in fields {
        print_field(&doc, field, 0, show_bbox)?;
    }

    Ok(())
}

fn print_field(doc: &Document, object: &Object, indent: usize, show_bbox: bool) -> Result<()> {
    let (_, field_obj) = doc.dereference(object)?;
    let field_dict = field_obj.as_dict()?;

    let name = field_dict
        .get(b"T")
        .ok()
        .map(|obj| object_to_string(doc, obj))
        .transpose()?;
    let value = field_dict
        .get(b"V")
        .ok()
        .map(|obj| object_to_string(doc, obj))
        .transpose()?;

    let indent_str = "  ".repeat(indent);
    let field_name = name.as_deref().unwrap_or("<unnamed>");
    let field_value = value.as_deref().unwrap_or("<empty>");
    println!("{indent_str}{} = {}", field_name, field_value);

    if show_bbox {
        let rects = collect_rects(doc, field_dict)?;
        for rect in rects {
            println!("{indent_str}  rect: {rect}");
        }
    }

    if let Ok(kids_obj) = field_dict.get(b"Kids").and_then(|obj| doc.dereference(obj).map(|(_, obj)| obj)) {
        if let Ok(kids) = kids_obj.as_array() {
            for kid in kids {
                print_field(doc, kid, indent + 1, show_bbox)?;
            }
        }
    }

    Ok(())
}

fn collect_rects(doc: &Document, field_dict: &lopdf::Dictionary) -> Result<Vec<String>> {
    let mut rects = Vec::new();

    if let Some(rect_obj) = field_dict.get(b"Rect").ok() {
        rects.push(object_to_string(doc, rect_obj)?);
    }

    if let Ok(kids_obj) = field_dict.get(b"Kids") {
        if let Ok((_, kids)) = doc.dereference(kids_obj) {
            if let Ok(kids_array) = kids.as_array() {
                for kid in kids_array {
                    if let Ok((_, kid_obj)) = doc.dereference(kid) {
                        if let Ok(kid_dict) = kid_obj.as_dict() {
                            rects.extend(collect_rects(doc, kid_dict)?);
                        }
                    }
                }
            }
        }
    }

    Ok(rects)
}

fn object_to_string(doc: &Document, object: &Object) -> Result<String> {
    let (_, object) = doc.dereference(object)?;

    let result = match object {
        Object::Null => "null".to_owned(),
        Object::Boolean(value) => value.to_string(),
        Object::Integer(value) => value.to_string(),
        Object::Real(value) => value.to_string(),
        Object::Name(name) => format!("/{}", String::from_utf8_lossy(name)),
        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
        Object::Array(array) => array
            .iter()
            .map(|item| object_to_string(doc, item))
            .collect::<Result<Vec<_>>>()?
            .join(", "),
        Object::Dictionary(_) => "<dictionary>".to_owned(),
        Object::Stream(_) => "<stream>".to_owned(),
        Object::Reference(_) => unreachable!("dereferenced reference should not remain"),
    };

    Ok(result)
}
