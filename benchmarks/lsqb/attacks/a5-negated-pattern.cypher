MATCH (tag1:Tag)<-[:HAS_TAG]-(message:Message)<-[:REPLY_OF]-(comment:Message {kind: 'Comment'})-[:HAS_TAG]->(tag2:Tag)
OPTIONAL MATCH (comment)-[h:HAS_TAG]->(tag1)
WITH tag1, tag2, h
WHERE tag1 <> tag2 AND h IS NULL
RETURN count(*) AS count
