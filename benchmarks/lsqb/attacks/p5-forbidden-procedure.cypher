MATCH (n)
CALL db.labels() YIELD label
RETURN label LIMIT 1
