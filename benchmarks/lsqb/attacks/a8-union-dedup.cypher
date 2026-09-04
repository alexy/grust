MATCH (p:Person)
RETURN count(*) AS count
UNION
MATCH (p:Person)
RETURN count(*) AS count
