-- The `sqlite` scenario's second session: the same database file, opened
-- again by a new process.
SELECT 'reopened: ' || count(*) || ' ' || (SELECT label FROM g WHERE grp = 6) FROM t;
SELECT 'integrity: ' || integrity_check FROM pragma_integrity_check;
SELECT 'pages: ' || (page_count > 10) FROM pragma_page_count;
