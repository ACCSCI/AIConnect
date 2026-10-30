import { DatabaseSync } from 'node:sqlite';
import { mkdirSync, chmodSync } from 'node:fs';
import { join } from 'node:path';

export class LocalStore {
  constructor(directory) {
    mkdirSync(directory, { recursive: true, mode: 0o700 });
    this.db = new DatabaseSync(join(directory, 'local.sqlite'));
    try { chmodSync(join(directory, 'local.sqlite'), 0o600); } catch {}
    this.db.exec(`PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA busy_timeout=3000;
      CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY, data TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS messages(id TEXT PRIMARY KEY, session_id TEXT NOT NULL, sender TEXT NOT NULL, text TEXT NOT NULL, created INTEGER NOT NULL, status TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id,created);`);
  }
  sessions() { return this.db.prepare('SELECT data FROM sessions ORDER BY rowid DESC').all().map(row => JSON.parse(row.data)); }
  session(id) { const row = this.db.prepare('SELECT data FROM sessions WHERE id=?').get(id); return row ? JSON.parse(row.data) : null; }
  putSession(session) { this.db.prepare('INSERT INTO sessions(id,data) VALUES (?,?) ON CONFLICT(id) DO UPDATE SET data=excluded.data').run(session.id, JSON.stringify(session)); }
  message(id) { return this.db.prepare('SELECT * FROM messages WHERE id=?').get(id); }
  messages(id) { return this.db.prepare('SELECT * FROM messages WHERE session_id=? ORDER BY created,rowid').all(id); }
  insertMessage(msg) {
    this.db.exec('BEGIN IMMEDIATE');
    try {
      const old = this.message(msg.id);
      if (old && (old.session_id !== msg.sessionId || old.sender !== msg.sender || old.text !== msg.text)) throw new Error('message_id_conflict');
      if (!old) this.db.prepare('INSERT INTO messages VALUES (?,?,?,?,?,?)').run(msg.id,msg.sessionId,msg.sender,msg.text,msg.created,msg.status);
      this.db.exec('COMMIT'); return !old;
    } catch (error) { this.db.exec('ROLLBACK'); throw error; }
  }
  mark(id,status) { this.db.prepare('UPDATE messages SET status=? WHERE id=?').run(status,id); }
  pending() { return this.db.prepare("SELECT * FROM messages WHERE status='pending' ORDER BY created").all(); }
  end(session,status) {
    session.status=status;this.putSession(session);
    this.db.prepare("UPDATE messages SET status=? WHERE session_id=? AND status='pending'").run(status,session.id);
  }
  close() { this.db.close(); }
}
