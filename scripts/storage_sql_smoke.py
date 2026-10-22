#!/usr/bin/env python3
"""Tiny no-build SQLite contract check against the schema embedded in storage.rs."""
from pathlib import Path
import re
import sqlite3

source = Path('src/storage.rs').read_text()
schema = re.search(r'conn\.execute_batch\("(PRAGMA journal_mode=WAL;.*?)"\)\?;', source, re.S).group(1)
c = sqlite3.connect(':memory:')
c.executescript(schema)
insert = re.search(r'"(INSERT INTO entries\(hash,kind,title,text,blob,source_app,created_at,last_seen,bytes\).*?)", params!', source, re.S).group(1)
args = ('hash', 'text', 'Hello', 'unique searchable text 中文', None, 'test', 1, 42)
def numbered(values):
    return {str(i): v for i, v in enumerate(values, 1)}

c.execute(insert, numbered(args))
id1 = c.execute('SELECT id FROM entries').fetchone()[0]
c.execute(insert, numbered(args))
assert c.execute('SELECT count(*) FROM entries').fetchone()[0] == 1
assert c.execute('SELECT count(*) FROM entries_fts WHERE entries_fts MATCH ?', ('"search"*',)).fetchone()[0] == 1
c.execute('INSERT INTO vectors VALUES (?,?,?,?)', (id1, 'test-model', 2, b'\0'*8))
c.execute('DELETE FROM entries')
assert c.execute('SELECT count(*) FROM vectors').fetchone()[0] == 0
assert c.execute('SELECT count(*) FROM entries_fts WHERE entries_fts MATCH ?', ('"search"*',)).fetchone()[0] == 0
c.execute(insert, numbered(('new', *args[1:])))
new_id = c.execute('SELECT id FROM entries').fetchone()[0]
assert new_id != id1
c.execute("UPDATE entries SET index_status=CASE WHEN attempts>=7 THEN 'failed' ELSE 'retry' END,index_error=?2,retry_at=?3+min(3600,30*(1 << min(attempts,7))),attempts=attempts+1 WHERE id=?1", numbered((new_id, 'safe error', 10)))
assert c.execute('SELECT index_status,retry_at,attempts FROM entries').fetchone() == ('retry', 40, 1)
print('PASS: actual schema, deduplication, FTS insert/delete triggers, vector cascade, non-reused IDs, retry SQL')
