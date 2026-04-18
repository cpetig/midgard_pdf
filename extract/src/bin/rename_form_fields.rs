use anyhow::{Context, Result};
use lopdf::{Document, Object};
use std::{env, path::PathBuf};

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let input_path = args
        .next()
        .context("Usage: rename_form_fields <input.pdf> [output.pdf]")?;
    let output_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| default_output_path(&input_path));

    let mut doc = Document::load(&input_path)?;
    let catalog = doc.catalog()?;
    let acro_form = catalog
        .get(b"AcroForm")
        .ok()
        .context("No AcroForm dictionary found")?;
    let acro_form = doc.dereference(acro_form)?.1.as_dict()?;
    let fields = acro_form
        .get(b"Fields")
        .ok()
        .context("No form fields found")?;
    let fields = doc.dereference(fields)?.1.as_array()?;
    let fields = fields.clone();

    for field in fields {
        rename_field(&mut doc, &field, None)?;
    }

    doc.save(&output_path)?;
    println!("Wrote renamed form fields to {}", output_path.display());
    Ok(())
}

fn rename_field(doc: &mut Document, object: &Object, prefix: Option<String>) -> Result<()> {
    let object_id = object.as_reference()?;

    let (current_name, is_text_field, kids_refs) = {
        let field_obj = doc.get_object(object_id)?;
        let field_dict = field_obj.as_dict()?;

        let current_name = field_dict
            .get(b"T")
            .ok()
            .map(|obj| object_to_string(doc, obj))
            .transpose()?;

        let is_text_field = field_dict
            .get(b"FT")
            .ok()
            .and_then(|obj| {
                let (_, obj) = doc.dereference(obj).ok()?;
                obj.as_name().ok().map(|name| name == b"Tx")
            })
            .unwrap_or(false);

        let kids_refs = if let Ok(kids_obj) = field_dict.get(b"Kids") {
            if let Ok((_, kids)) = doc.dereference(kids_obj) {
                if let Ok(kids_array) = kids.as_array() {
                    kids_array.clone()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        (current_name, is_text_field, kids_refs)
    };

    let full_name = make_full_name(prefix, current_name);

    if is_text_field {
        let field_obj = doc.get_object_mut(object_id)?;
        let field_dict = field_obj.as_dict_mut()?;
        field_dict.set(b"V", Object::string_literal(full_name.clone()));
    }

    for kid in kids_refs {
        rename_field(doc, &kid, Some(full_name.clone()))?;
    }

    Ok(())
}

fn make_full_name(prefix: Option<String>, current: Option<String>) -> String {
    match (prefix, current) {
        (Some(prefix), Some(current)) => format!("{prefix}.{current}"),
        (Some(prefix), None) => prefix,
        (None, Some(current)) => current,
        (None, None) => "<unnamed>".to_string(),
    }
}

fn object_to_string(doc: &Document, object: &Object) -> Result<String> {
    let (_, object) = doc.dereference(object)?;

    let result = match object {
        Object::Null => "null".to_string(),
        Object::Boolean(value) => value.to_string(),
        Object::Integer(value) => value.to_string(),
        Object::Real(value) => value.to_string(),
        Object::Name(name) => String::from_utf8_lossy(name).into_owned(),
        Object::String(bytes, _) => String::from_utf8_lossy(bytes).into_owned(),
        Object::Array(array) => array
            .iter()
            .map(|item| object_to_string(doc, item))
            .collect::<Result<Vec<_>>>()?
            .join(", "),
        Object::Dictionary(_) => "<dictionary>".to_string(),
        Object::Stream(_) => "<stream>".to_string(),
        Object::Reference(_) => unreachable!("dereferenced reference should not remain"),
    };

    Ok(result)
}

fn default_output_path(input: &str) -> PathBuf {
    let path = PathBuf::from(input);
    let file_stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("renamed_form");
    let out_name = format!("{}_renamed.pdf", file_stem);
    if let Some(parent) = path.parent() {
        parent.join(out_name)
    } else {
        PathBuf::from(out_name)
    }
}
