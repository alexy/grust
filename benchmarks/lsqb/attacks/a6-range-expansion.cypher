UNWIND range(1, 10000) AS i
RETURN count(*) AS count

