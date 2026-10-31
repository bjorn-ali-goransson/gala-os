-- After the concurrent writers: every row of both is there.
SELECT 'concurrent: ' || count(*) || ' rows, ' || count(DISTINCT n) || ' distinct, sum ' || sum(n) FROM w;
SELECT 'integrity after writers: ' || integrity_check FROM pragma_integrity_check;
