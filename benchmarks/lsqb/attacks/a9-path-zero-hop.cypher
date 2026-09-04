MATCH (person:Person)-[:KNOWS*0..0]->(same:Person)
RETURN count(*) AS count
