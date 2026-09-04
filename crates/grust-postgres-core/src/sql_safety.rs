use grust_core::prelude::*;

pub const POSTGRES_IDENTIFIER_MAX_BYTES: usize = 63;

pub(crate) fn validate_identifier_length(value: &str) -> Result<()> {
    let length = value.len();
    if length > POSTGRES_IDENTIFIER_MAX_BYTES {
        return Err(GrustError::Schema(format!(
            "PostgreSQL identifier '{value}' is {length} bytes; the limit is {POSTGRES_IDENTIFIER_MAX_BYTES} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn validate_typed_column_alias(alias: &str) -> Result<()> {
    if alias.is_empty() || alias.contains('\0') {
        return Err(GrustError::Schema(format!(
            "invalid PostgreSQL typed field alias {alias:?}"
        )));
    }
    let length = alias.len();
    if length > POSTGRES_IDENTIFIER_MAX_BYTES {
        return Err(GrustError::Schema(format!(
            "PostgreSQL typed field alias '{alias}' is {length} bytes; the limit is {POSTGRES_IDENTIFIER_MAX_BYTES} bytes"
        )));
    }
    Ok(())
}

/// Reject SQL that can take ownership of the shared connection's transaction
/// state. Words inside quoted strings, quoted identifiers, dollar-quoted
/// bodies, and comments are skipped by the scanner.
pub(crate) fn validate_autocommit_sql(sql: &str) -> Result<()> {
    if sql.contains('\0') {
        return Err(invalid_sql("NUL byte"));
    }

    let mut scanner = SqlScanner::new(sql);
    let mut prefix = Vec::new();
    let mut classified_safe = false;
    while let Some(item) = scanner.next_item()? {
        match item {
            SqlItem::StatementEnd => {
                prefix.clear();
                classified_safe = false;
            }
            SqlItem::Other => classified_safe = true,
            SqlItem::Word(word) if !classified_safe => {
                prefix.push(word.to_ascii_uppercase());
                match classify_statement_prefix(&prefix) {
                    PrefixClassification::Forbidden(command) => {
                        return Err(GrustError::Unsupported(format!(
                            "PostgreSQL execute is autocommit-only; transaction-control statement '{command}' is not allowed"
                        )));
                    }
                    PrefixClassification::Safe => classified_safe = true,
                    PrefixClassification::NeedMore => {}
                }
            }
            SqlItem::Word(_) => {}
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrefixClassification {
    Forbidden(&'static str),
    Safe,
    NeedMore,
}

fn classify_statement_prefix(words: &[String]) -> PrefixClassification {
    let first = words[0].as_str();
    match first {
        "ABORT" | "BEGIN" | "COMMIT" | "END" | "ROLLBACK" | "SAVEPOINT" => {
            PrefixClassification::Forbidden(first_keyword_name(first))
        }
        "RELEASE" => PrefixClassification::Forbidden("RELEASE SAVEPOINT"),
        "START" => PrefixClassification::Forbidden("START TRANSACTION"),
        "PREPARE" => match words.get(1).map(String::as_str) {
            None => PrefixClassification::NeedMore,
            Some("TRANSACTION") => PrefixClassification::Forbidden("PREPARE TRANSACTION"),
            Some(_) => PrefixClassification::Safe,
        },
        "SET" => classify_set_prefix(words),
        _ => PrefixClassification::Safe,
    }
}

fn first_keyword_name(keyword: &str) -> &'static str {
    match keyword {
        "ABORT" => "ABORT",
        "BEGIN" => "BEGIN",
        "COMMIT" => "COMMIT",
        "END" => "END",
        "ROLLBACK" => "ROLLBACK",
        "SAVEPOINT" => "SAVEPOINT",
        _ => unreachable!("known transaction keyword"),
    }
}

fn classify_set_prefix(words: &[String]) -> PrefixClassification {
    match words.get(1).map(String::as_str) {
        None => PrefixClassification::NeedMore,
        Some("TRANSACTION") => PrefixClassification::Forbidden("SET TRANSACTION"),
        Some("LOCAL" | "SESSION") => match words.get(2).map(String::as_str) {
            None => PrefixClassification::NeedMore,
            Some("TRANSACTION") => PrefixClassification::Forbidden("SET TRANSACTION"),
            Some("CHARACTERISTICS") if words[1] == "SESSION" => {
                match words.get(3).map(String::as_str) {
                    None => PrefixClassification::NeedMore,
                    Some("AS") => match words.get(4).map(String::as_str) {
                        None => PrefixClassification::NeedMore,
                        Some("TRANSACTION") => PrefixClassification::Forbidden(
                            "SET SESSION CHARACTERISTICS AS TRANSACTION",
                        ),
                        Some(_) => PrefixClassification::Safe,
                    },
                    Some(_) => PrefixClassification::Safe,
                }
            }
            Some(_) => PrefixClassification::Safe,
        },
        Some(_) => PrefixClassification::Safe,
    }
}

fn invalid_sql(detail: &str) -> GrustError {
    GrustError::Unsupported(format!(
        "PostgreSQL execute received invalid SQL while enforcing its autocommit-only contract: {detail}"
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SqlItem<'a> {
    Word(&'a str),
    StatementEnd,
    Other,
}

struct SqlScanner<'a> {
    sql: &'a str,
    bytes: &'a [u8],
    index: usize,
}

impl<'a> SqlScanner<'a> {
    fn new(sql: &'a str) -> Self {
        Self {
            sql,
            bytes: sql.as_bytes(),
            index: 0,
        }
    }

    fn next_item(&mut self) -> Result<Option<SqlItem<'a>>> {
        self.skip_whitespace_and_comments()?;
        if self.index >= self.bytes.len() {
            return Ok(None);
        }

        let start = self.index;
        match self.bytes[self.index] {
            b';' => {
                self.index += 1;
                Ok(Some(SqlItem::StatementEnd))
            }
            b'\'' => {
                let escape_backslashes = self.has_escape_string_prefix();
                self.skip_single_quoted(escape_backslashes)?;
                Ok(Some(SqlItem::Other))
            }
            b'"' => {
                self.skip_double_quoted()?;
                Ok(Some(SqlItem::Other))
            }
            b'$' => {
                if !self.skip_dollar_quoted()? {
                    self.index += 1;
                }
                Ok(Some(SqlItem::Other))
            }
            byte if is_word_start(byte) => {
                self.index += 1;
                while self
                    .bytes
                    .get(self.index)
                    .is_some_and(|byte| is_word_continue(*byte))
                {
                    self.index += 1;
                }
                Ok(Some(SqlItem::Word(&self.sql[start..self.index])))
            }
            _ => {
                self.index += 1;
                Ok(Some(SqlItem::Other))
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<()> {
        loop {
            while self
                .bytes
                .get(self.index)
                .is_some_and(|byte| byte.is_ascii_whitespace())
            {
                self.index += 1;
            }
            if self.bytes.get(self.index..self.index + 3) == Some(&[0xef, 0xbb, 0xbf]) {
                self.index += 3;
                continue;
            }
            if self.bytes.get(self.index..self.index + 2) == Some(b"--") {
                self.index += 2;
                while self
                    .bytes
                    .get(self.index)
                    .is_some_and(|byte| *byte != b'\n' && *byte != b'\r')
                {
                    self.index += 1;
                }
                continue;
            }
            if self.bytes.get(self.index..self.index + 2) == Some(b"/*") {
                self.skip_block_comment()?;
                continue;
            }
            return Ok(());
        }
    }

    fn skip_block_comment(&mut self) -> Result<()> {
        self.index += 2;
        let mut depth = 1usize;
        while self.index < self.bytes.len() {
            if self.bytes.get(self.index..self.index + 2) == Some(b"/*") {
                depth += 1;
                self.index += 2;
            } else if self.bytes.get(self.index..self.index + 2) == Some(b"*/") {
                depth -= 1;
                self.index += 2;
                if depth == 0 {
                    return Ok(());
                }
            } else {
                self.index += 1;
            }
        }
        Err(invalid_sql("unterminated block comment"))
    }

    fn has_escape_string_prefix(&self) -> bool {
        self.index > 0
            && matches!(self.bytes[self.index - 1], b'e' | b'E')
            && (self.index == 1 || !is_word_continue(self.bytes[self.index - 2]))
    }

    fn skip_single_quoted(&mut self, escape_backslashes: bool) -> Result<()> {
        self.index += 1;
        while self.index < self.bytes.len() {
            match self.bytes[self.index] {
                b'\\' if escape_backslashes => {
                    self.index += 1;
                    if self.index < self.bytes.len() {
                        self.index += 1;
                    }
                }
                b'\'' if self.bytes.get(self.index + 1) == Some(&b'\'') => {
                    self.index += 2;
                }
                b'\'' => {
                    self.index += 1;
                    return Ok(());
                }
                _ => self.index += 1,
            }
        }
        Err(invalid_sql("unterminated quoted string"))
    }

    fn skip_double_quoted(&mut self) -> Result<()> {
        self.index += 1;
        while self.index < self.bytes.len() {
            if self.bytes[self.index] == b'"' {
                if self.bytes.get(self.index + 1) == Some(&b'"') {
                    self.index += 2;
                } else {
                    self.index += 1;
                    return Ok(());
                }
            } else {
                self.index += 1;
            }
        }
        Err(invalid_sql("unterminated quoted identifier"))
    }

    fn skip_dollar_quoted(&mut self) -> Result<bool> {
        let delimiter_start = self.index;
        let mut delimiter_end = self.index + 1;
        if self.bytes.get(delimiter_end) == Some(&b'$') {
            delimiter_end += 1;
        } else {
            let Some(first) = self.bytes.get(delimiter_end).copied() else {
                return Ok(false);
            };
            if !is_tag_start(first) {
                return Ok(false);
            }
            delimiter_end += 1;
            while self
                .bytes
                .get(delimiter_end)
                .is_some_and(|byte| is_tag_continue(*byte))
            {
                delimiter_end += 1;
            }
            if self.bytes.get(delimiter_end) != Some(&b'$') {
                return Ok(false);
            }
            delimiter_end += 1;
        }

        let delimiter = &self.bytes[delimiter_start..delimiter_end];
        self.index = delimiter_end;
        while self.index + delimiter.len() <= self.bytes.len() {
            if &self.bytes[self.index..self.index + delimiter.len()] == delimiter {
                self.index += delimiter.len();
                return Ok(true);
            }
            self.index += 1;
        }
        Err(invalid_sql("unterminated dollar-quoted string"))
    }
}

fn is_word_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || !byte.is_ascii()
}

fn is_word_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$') || !byte.is_ascii()
}

fn is_tag_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_'
}

fn is_tag_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autocommit_guard_rejects_transaction_control_in_batches() {
        for sql in [
            "BEGIN",
            "ABORT",
            "END WORK",
            "COMMIT AND CHAIN",
            "ROLLBACK TO SAVEPOINT before_change",
            "SAVEPOINT before_change",
            "RELEASE SAVEPOINT before_change",
            "START /* gap */ TRANSACTION READ WRITE",
            "PREPARE -- gap\n TRANSACTION 'prepared-id'",
            "SET TRANSACTION READ ONLY",
            "SET LOCAL TRANSACTION READ WRITE",
            "SET SESSION TRANSACTION ISOLATION LEVEL SERIALIZABLE",
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY",
            "SELECT 1; /* between statements */ COMMIT",
        ] {
            let error = validate_autocommit_sql(sql)
                .expect_err("transaction control must not reach the shared connection");
            assert!(error.to_string().contains("autocommit-only"), "{sql}");
        }
    }

    #[test]
    fn autocommit_guard_ignores_keywords_in_sql_literals_and_comments() {
        for sql in [
            "SELECT 'BEGIN; COMMIT; ROLLBACK'",
            "SELECT E'quote: \\' COMMIT; still text'",
            "SELECT \"BEGIN\" FROM \"COMMIT\"",
            "SELECT $$BEGIN; COMMIT$$",
            "SELECT $body$BEGIN; SAVEPOINT x; COMMIT$body$",
            "-- BEGIN; COMMIT\nSELECT 1",
            "/* ROLLBACK /* SAVEPOINT */ COMMIT */ SELECT 1",
            "CREATE PROCEDURE p() LANGUAGE plpgsql AS $$ BEGIN COMMIT; END $$",
            "SELECT begin FROM transaction_log",
            "SET search_path = public",
            "PREPARE query AS SELECT 'COMMIT'",
        ] {
            validate_autocommit_sql(sql)
                .unwrap_or_else(|error| panic!("safe SQL was rejected: {sql}: {error}"));
        }
    }

    #[test]
    fn autocommit_guard_rejects_unterminated_lexical_regions_and_nul() {
        for sql in [
            "SELECT 'unterminated",
            "SELECT \"unterminated",
            "SELECT $tag$body",
            "/* open",
            "SELECT '\0'",
        ] {
            assert!(validate_autocommit_sql(sql).is_err(), "{sql:?}");
        }
    }
}
