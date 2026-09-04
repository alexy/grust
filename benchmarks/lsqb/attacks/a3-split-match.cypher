MATCH (:Tag)<-[:HAS_TAG]-(message:Message)-[:HAS_CREATOR]->(creator:Person)
MATCH (message)<-[:LIKES]-(liker:Person)
MATCH (message)<-[:REPLY_OF]-(comment:Message {kind: 'Comment'})
RETURN count(*) AS count

