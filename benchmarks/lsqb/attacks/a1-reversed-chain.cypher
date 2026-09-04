MATCH (:TagClass)<-[:HAS_TYPE]-(:Tag)<-[:HAS_TAG]-(:Message {kind: 'Comment'})-[:REPLY_OF]->(:Message {kind: 'Post'})<-[:CONTAINER_OF]-(:Forum)-[:HAS_MEMBER]->(:Person)-[:IS_LOCATED_IN]->(:City)-[:IS_PART_OF]->(:Country)
RETURN count(*) AS count

