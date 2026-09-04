/* MATCH (ignored) DELETE ignored RETURN 0 */
MATCH /* delimiter pressure */ (node)
// RETURN and UNION in comments must remain trivia
RETURN (((count(node)))) AS count
