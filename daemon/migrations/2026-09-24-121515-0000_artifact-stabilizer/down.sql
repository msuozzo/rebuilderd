PRAGMA foreign_keys= OFF;

CREATE TABLE _new_rebuild_artifacts
(
    id                 INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
    rebuild_id         INTEGER NOT NULL REFERENCES rebuilds ON DELETE CASCADE,
    name               TEXT    NOT NULL,
    diffoscope_log_id  INTEGER REFERENCES diffoscope_logs ON DELETE SET NULL,
    attestation_log_id INTEGER REFERENCES attestation_logs ON DELETE SET NULL,
    status             TEXT
);

INSERT INTO _new_rebuild_artifacts (id, rebuild_id, name, diffoscope_log_id, attestation_log_id, status)
SELECT id,
       rebuild_id,
       name,
       diffoscope_log_id,
       attestation_log_id,
       status
FROM rebuild_artifacts;

DROP TABLE rebuild_artifacts;
ALTER TABLE _new_rebuild_artifacts
    RENAME TO rebuild_artifacts;

CREATE INDEX rebuild_artifacts_rebuild_id_idx ON rebuild_artifacts (rebuild_id);
CREATE INDEX rebuild_artifacts_status_idx ON rebuild_artifacts (status);

PRAGMA foreign_keys= ON;
