/* The `sqlite` scenario's driver (user/sqlite/sqlite.sh): runs the SQL
 * file argv[2] on the database file argv[1] with sqlite3_exec, printing
 * each result row's columns joined by '|'. SQLite's own shell (shell.c)
 * is not used: on Windows it needs either MSVC or mingw's dirent
 * functions, which TinyCC (linking msvcrt only) does not provide.
 * Exits 1 at the first error, printing it. */
#include <stdio.h>
#include <stdlib.h>
#include "sqlite3.h"

static int row(void *unused, int n, char **values, char **names)
{
    int i;
    (void)unused;
    (void)names;
    for (i = 0; i < n; i++)
        printf("%s%s", i ? "|" : "", values[i] ? values[i] : "NULL");
    printf("\n");
    return 0;
}

int main(int argc, char **argv)
{
    sqlite3 *db;
    FILE *f;
    long size;
    char *sql, *err = NULL;

    if (argc == 2 && argv[1][0] == '-') {
        printf("%s\n", sqlite3_libversion());
        return 0;
    }
    if (argc != 3) {
        fprintf(stderr, "usage: sqlrun database file.sql | sqlrun -version\n");
        return 2;
    }
    f = fopen(argv[2], "rb");
    if (!f) {
        perror(argv[2]);
        return 1;
    }
    fseek(f, 0, SEEK_END);
    size = ftell(f);
    fseek(f, 0, SEEK_SET);
    sql = malloc(size + 1);
    if (!sql || fread(sql, 1, size, f) != (size_t)size) {
        fprintf(stderr, "cannot read %s\n", argv[2]);
        return 1;
    }
    sql[size] = 0;
    fclose(f);
    if (sqlite3_open(argv[1], &db) != SQLITE_OK) {
        fprintf(stderr, "open: %s\n", sqlite3_errmsg(db));
        return 1;
    }
    if (sqlite3_exec(db, sql, row, NULL, &err) != SQLITE_OK) {
        printf("error: %s\n", err);
        sqlite3_close(db);
        return 1;
    }
    sqlite3_close(db);
    free(sql);
    return 0;
}
