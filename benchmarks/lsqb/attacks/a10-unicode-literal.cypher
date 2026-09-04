MATCH (person:Person)
WHERE 'é' = '\u00e9'
RETURN count(*) AS `résultat_🦀`
