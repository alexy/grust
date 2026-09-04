MATCH (n)
WHERE n.source_type = '\u{110000}'
RETURN n LIMIT 1
