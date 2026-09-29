ALTER TABLE devices ADD COLUMN revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN(0,1));
ALTER TABLE agents ADD COLUMN revoked INTEGER NOT NULL DEFAULT 0 CHECK(revoked IN(0,1));
CREATE TABLE agent_sessions(token_hash TEXT PRIMARY KEY,user_id TEXT NOT NULL,device_id TEXT NOT NULL,agent_id TEXT NOT NULL,space_id TEXT NOT NULL REFERENCES spaces(id),can_write INTEGER NOT NULL CHECK(can_write IN(0,1)),expires_at INTEGER NOT NULL,FOREIGN KEY(user_id,agent_id) REFERENCES agents(user_id,id),FOREIGN KEY(user_id,device_id) REFERENCES devices(user_id,id));
ALTER TABLE memory_denials RENAME TO memory_denials_old;
CREATE TABLE memory_denials(space_id TEXT NOT NULL REFERENCES spaces(id),user_id TEXT NOT NULL REFERENCES users(id),kind TEXT NOT NULL,logical_key TEXT NOT NULL,action TEXT NOT NULL CHECK(action IN('create','update','delete','compact','rollback','export','*')),PRIMARY KEY(space_id,user_id,kind,logical_key,action));
INSERT INTO memory_denials SELECT * FROM memory_denials_old;
DROP TABLE memory_denials_old;
