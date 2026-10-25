CREATE TABLE vectors (
 fingerprint TEXT NOT NULL, input_hash TEXT NOT NULL, vector BLOB NOT NULL,
 PRIMARY KEY(fingerprint,input_hash)
) WITHOUT ROWID;
