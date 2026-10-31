-- The `sqlite` scenario's first session (user/sqlite/sqlite.sh): a file
-- database built and queried by sqlrun.exe (SQLite compiled on gala). One
-- labelled line per result.
CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, grp INTEGER, val REAL);
WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 1000)
INSERT INTO t(id, name, grp, val) SELECT x, 'n' || x, x % 7, x * 1.5 FROM c;
CREATE INDEX t_grp ON t(grp);
CREATE TABLE g(grp INTEGER PRIMARY KEY, label TEXT);
INSERT INTO g VALUES (0, 'zero'), (1, 'one'), (2, 'two'), (3, 'three'),
    (4, 'four'), (5, 'five'), (6, 'six');
SELECT 'count: ' || count(*) || ' sum: ' || sum(id) || ' total: ' || total(val) FROM t;
SELECT 'groups: ' || group_concat(label || '=' || n, ',') FROM
    (SELECT g.label, count(*) AS n FROM t JOIN g USING (grp) GROUP BY g.grp ORDER BY g.grp);
SELECT 'avg: ' || printf('%.3f', avg(val)) FROM t WHERE grp = 3;
SELECT 'indexed: ' || count(*) || ' ' || min(name) || ' ' || max(id) FROM t WHERE grp = 5;
BEGIN;
DELETE FROM t WHERE id > 10;
SELECT 'in transaction: ' || count(*) FROM t;
ROLLBACK;
SELECT 'after rollback: ' || count(*) FROM t;
SELECT 'printf: ' || printf('%5d|%-5s|%08.3f|%e|%,d', 42, 'ab', 3.14159, 12345.678, 1234567);
SELECT 'strings: ' || upper('gala') || ' ' || substr('sqlite on gala', 8) || ' '
    || replace('a-b-c', '-', '+') || ' ' || length('h' || char(233) || 'llo');
SELECT 'json: ' || json_object('n', 3, 'list', json_array(1, 2.5, 'x'));
