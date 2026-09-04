MATCH (post:Message {kind: 'Post'})-[:HAS_CREATOR]->(person2:Person),
      (comment:Message {kind: 'Comment'})-[:REPLY_OF]->(post),
      (comment)-[:HAS_CREATOR]->(person1:Person),
      (person2)-[:KNOWS]-(person1)
RETURN count(*) AS count

