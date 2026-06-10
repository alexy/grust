use grust::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zod_rs::prelude::{Schema, object, string};

#[derive(Debug, Deserialize, Serialize, garde::Validate)]
#[garde(allow_unvalidated)]
struct Person {
    #[garde(length(min = 1))]
    id: String,
    #[garde(length(min = 1))]
    name: String,
    #[garde(length(min = 2))]
    skills: Vec<String>,
}

fn main() -> Result<()> {
    let schema = object()
        .field("id", string().min(1))
        .field("name", string().min(1))
        .field("skills", string().array())
        .strict();

    let shape_error = parse_typed_json::<Person, _>(
        &schema,
        &json!({
            "id": "nia",
            "name": "Nia",
            "skills": "rust"
        }),
    )
    .expect_err("zod-rs should reject this before serde");
    println!("shape error: {shape_error}");

    let domain_error = parse_typed_json::<Person, _>(
        &schema,
        &json!({
            "id": "nia",
            "name": "Nia",
            "skills": ["rust"]
        }),
    )
    .expect_err("garde should reject this after serde");
    println!("domain error: {domain_error}");

    Ok(())
}
