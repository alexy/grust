use grust_core::Result;

use crate::validate_arrow_view_name;

pub(crate) fn drop_sql(name: &str) -> Result<String> {
    validate_arrow_view_name(name)?;
    Ok(format!("DROP VIEW IF EXISTS `{name}`"))
}

#[cfg(test)]
#[path = "temp_view/tests.rs"]
mod tests;
