-- The `sqlite` scenario's concurrent writers (user/sqlite/sqlite.sh): two
-- processes run this at the same time on one database. Each INSERT is its
-- own transaction, so the writers take the file's locks in turns; the
-- busy timeout makes SQLite retry (its busy handler) instead of failing
-- with SQLITE_BUSY while the other one holds the lock.
PRAGMA busy_timeout = 30000;
INSERT INTO w(n) VALUES (1);
INSERT INTO w(n) VALUES (2);
INSERT INTO w(n) VALUES (3);
INSERT INTO w(n) VALUES (4);
INSERT INTO w(n) VALUES (5);
INSERT INTO w(n) VALUES (6);
INSERT INTO w(n) VALUES (7);
INSERT INTO w(n) VALUES (8);
INSERT INTO w(n) VALUES (9);
INSERT INTO w(n) VALUES (10);
INSERT INTO w(n) VALUES (11);
INSERT INTO w(n) VALUES (12);
INSERT INTO w(n) VALUES (13);
INSERT INTO w(n) VALUES (14);
INSERT INTO w(n) VALUES (15);
INSERT INTO w(n) VALUES (16);
INSERT INTO w(n) VALUES (17);
INSERT INTO w(n) VALUES (18);
INSERT INTO w(n) VALUES (19);
INSERT INTO w(n) VALUES (20);
INSERT INTO w(n) VALUES (21);
INSERT INTO w(n) VALUES (22);
INSERT INTO w(n) VALUES (23);
INSERT INTO w(n) VALUES (24);
INSERT INTO w(n) VALUES (25);
INSERT INTO w(n) VALUES (26);
INSERT INTO w(n) VALUES (27);
INSERT INTO w(n) VALUES (28);
INSERT INTO w(n) VALUES (29);
INSERT INTO w(n) VALUES (30);
INSERT INTO w(n) VALUES (31);
INSERT INTO w(n) VALUES (32);
INSERT INTO w(n) VALUES (33);
INSERT INTO w(n) VALUES (34);
INSERT INTO w(n) VALUES (35);
INSERT INTO w(n) VALUES (36);
INSERT INTO w(n) VALUES (37);
INSERT INTO w(n) VALUES (38);
INSERT INTO w(n) VALUES (39);
INSERT INTO w(n) VALUES (40);
