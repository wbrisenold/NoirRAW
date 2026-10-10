use super::*;
use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;
use anyhow::Result;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS images (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        path TEXT NOT NULL UNIQUE,
        filename TEXT NOT NULL,
        sha256 TEXT NOT NULL,
        width INTEGER NOT NULL,
        height INTEGER NOT NULL,
        file_size INTEGER NOT NULL,
        modified_time INTEGER NOT NULL,
        imported_time INTEGER NOT NULL,
        rating INTEGER DEFAULT 0,
        color_label INTEGER DEFAULT 0,
        pick_reject INTEGER DEFAULT 0,
        has_raw INTEGER DEFAULT 0,
        mime_type TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_images_sha256 ON images(sha256);
    CREATE INDEX IF NOT EXISTS idx_images_path ON images(path);
    CREATE INDEX IF NOT EXISTS idx_images_rating ON images(rating);
    CREATE INDEX IF NOT EXISTS idx_images_pick_reject ON images(pick_reject);

    CREATE TABLE IF NOT EXISTS faces (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        image_id INTEGER NOT NULL REFERENCES images(id) ON DELETE CASCADE,
        bbox_x REAL NOT NULL,
        bbox_y REAL NOT NULL,
        bbox_w REAL NOT NULL,
        bbox_h REAL NOT NULL,
        landmarks TEXT, -- JSON array of 5 [x,y] points
        embedding BLOB, -- 512 * 4 bytes = 2048 bytes
        person_id INTEGER REFERENCES persons(id) ON DELETE SET NULL,
        confidence REAL NOT NULL,
        created_time INTEGER NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_faces_image_id ON faces(image_id);
    CREATE INDEX IF NOT EXISTS idx_faces_person_id ON faces(person_id);

    CREATE TABLE IF NOT EXISTS persons (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL DEFAULT '',
        face_count INTEGER DEFAULT 0,
        avatar_face_id INTEGER REFERENCES faces(id) ON DELETE SET NULL,
        created_time INTEGER NOT NULL,
        updated_time INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS library_folders (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        path TEXT NOT NULL UNIQUE,
        name TEXT NOT NULL,
        image_count INTEGER DEFAULT 0,
        last_scanned INTEGER,
        created_time INTEGER NOT NULL
    );

    CREATE TABLE IF NOT EXISTS metadata_cache (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_time INTEGER NOT NULL
    );
";

pub struct LibraryDb {
    conn: Connection,
}

impl LibraryDb {
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn insert_image(&self, img: &LibraryImage) -> SqlResult<i64> {
        self.conn.execute(
            "INSERT INTO images (path, filename, sha256, width, height, file_size, modified_time, imported_time, rating, color_label, pick_reject, has_raw, mime_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                img.path.to_string_lossy().to_string(),
                img.filename,
                img.sha256,
                img.width as i32,
                img.height as i32,
                img.file_size as i64,
                img.modified_time,
                img.imported_time,
                img.rating,
                img.color_label,
                img.pick_reject,
                img.has_raw as i32,
                img.mime_type,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn update_image(&self, img: &LibraryImage) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images SET rating=?1, color_label=?2, pick_reject=?3 WHERE id=?4",
            params![img.rating, img.color_label, img.pick_reject, img.id],
        )?;
        Ok(())
    }

    pub fn update_image_metadata(&self, id: i64, rating: i16, color_label: i16, pick_reject: i16) -> SqlResult<()> {
        self.conn.execute(
            "UPDATE images SET rating=?1, color_label=?2, pick_reject=?3 WHERE id=?4",
            params![rating, color_label, pick_reject, id],
        )?;
        Ok(())
    }

    pub fn get_image(&self, id: i64) -> SqlResult<Option<LibraryImage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, filename, sha256, width, height, file_size, modified_time, imported_time, rating, color_label, pick_reject, has_raw, mime_type FROM images WHERE id=?1"
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_image(row)))
        } else {
            Ok(None)
        }
    }

    pub fn get_image_by_sha256(&self, sha256: &str) -> SqlResult<Option<LibraryImage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, filename, sha256, width, height, file_size, modified_time, imported_time, rating, color_label, pick_reject, has_raw, mime_type FROM images WHERE sha256=?1"
        )?;
        let mut rows = stmt.query(params![sha256])?;
        if let Some(row) = rows.next()? {
            Ok(Some(self.row_to_image(row)))
        } else {
            Ok(None)
        }
    }

    pub fn list_images(&self, limit: usize, offset: usize) -> SqlResult<Vec<LibraryImage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, filename, sha256, width, height, file_size, modified_time, imported_time, rating, color_label, pick_reject, has_raw, mime_type FROM images ORDER BY modified_time DESC LIMIT ?1 OFFSET ?2"
        )?;
        let rows = stmt.query_map(params![limit as i32, offset as i32], |row| Ok(self.row_to_image(row)))?;
        rows.collect()
    }

    pub fn list_images_filtered(&self, filter: &str, limit: usize, offset: usize) -> SqlResult<Vec<LibraryImage>> {
        let sql = match filter {
            "picked" => "SELECT ... FROM images WHERE pick_reject=1 ORDER BY modified_time DESC LIMIT ?1 OFFSET ?2",
            "rejected" => "SELECT ... FROM images WHERE pick_reject=-1 ORDER BY modified_time DESC LIMIT ?1 OFFSET ?2",
            "rated" => "SELECT ... FROM images WHERE rating>0 ORDER BY modified_time DESC LIMIT ?1 OFFSET ?2",
            "color_labeled" => "SELECT ... FROM images WHERE color_label>0 ORDER BY modified_time DESC LIMIT ?1 OFFSET ?2",
            _ => "SELECT ... FROM images ORDER BY modified_time DESC LIMIT ?1 OFFSET ?2",
        }.replace("...", "id, path, filename, sha256, width, height, file_size, modified_time, imported_time, rating, color_label, pick_reject, has_raw, mime_type");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![limit as i32, offset as i32], |row| Ok(self.row_to_image(row)))?;
        rows.collect()
    }

    pub fn count_images(&self, filter: &str) -> SqlResult<i64> {
        let sql = match filter {
            "picked" => "SELECT COUNT(*) FROM images WHERE pick_reject=1",
            "rejected" => "SELECT COUNT(*) FROM images WHERE pick_reject=-1",
            "rated" => "SELECT COUNT(*) FROM images WHERE rating>0",
            "color_labeled" => "SELECT COUNT(*) FROM images WHERE color_label>0",
            _ => "SELECT COUNT(*) FROM images",
        };
        self.conn.query_row(sql, [], |row| row.get(0))
    }

    fn row_to_image(&self, row: &rusqlite::Row) -> LibraryImage {
        LibraryImage {
            id: row.get(0).unwrap(),
            path: PathBuf::from(row.get::<_, String>(1).unwrap()),
            filename: row.get(2).unwrap(),
            sha256: row.get(3).unwrap(),
            width: row.get::<_, i32>(4).unwrap() as u32,
            height: row.get::<_, i32>(5).unwrap() as u32,
            file_size: row.get::<_, i64>(6).unwrap() as u64,
            modified_time: row.get(7).unwrap(),
            imported_time: row.get(8).unwrap(),
            rating: row.get(9).unwrap(),
            color_label: row.get(10).unwrap(),
            pick_reject: row.get(11).unwrap(),
            has_raw: row.get::<_, i32>(12).unwrap() != 0,
            mime_type: row.get(13).unwrap(),
        }
    }

    // Face operations
    pub fn insert_face(&self, face: &Face) -> SqlResult<i64> {
        let landmarks_json = face.landmarks.map(|l| serde_json::to_string(&l).unwrap());
        let embedding_bytes = face.embedding.as_ref().map(|e| bytemuck::cast_slice(e).to_vec());

        self.conn.execute(
            "INSERT INTO faces (image_id, bbox_x, bbox_y, bbox_w, bbox_h, landmarks, embedding, person_id, confidence, created_time)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                face.image_id,
                face.bbox[0], face.bbox[1], face.bbox[2], face.bbox[3],
                landmarks_json,
                embedding_bytes,
                face.person_id,
                face.confidence,
                chrono::Utc::now().timestamp(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_faces_for_image(&self, image_id: i64) -> SqlResult<Vec<Face>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, image_id, bbox_x, bbox_y, bbox_w, bbox_h, landmarks, embedding, person_id, confidence FROM faces WHERE image_id=?1"
        )?;
        let rows = stmt.query_map(params![image_id], |row| {
            let landmarks: Option<String> = row.get(6)?;
            let embedding: Option<Vec<u8>> = row.get(7)?;
            Ok(Face {
                id: row.get(0)?,
                image_id: row.get(1)?,
                bbox: [row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?],
                landmarks: landmarks.and_then(|s| serde_json::from_str(&s).ok()),
                embedding: embedding.map(|b| bytemuck::cast_slice(&b).to_vec()),
                person_id: row.get(8)?,
                confidence: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn update_face_person(&self, face_id: i64, person_id: Option<i64>) -> SqlResult<()> {
        self.conn.execute("UPDATE faces SET person_id=?1 WHERE id=?2", params![person_id, face_id])?;
        Ok(())
    }

    // Person operations
    pub fn insert_person(&self, name: &str) -> SqlResult<i64> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO persons (name, face_count, created_time, updated_time) VALUES (?1, 0, ?2, ?2)",
            params![name, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_person(&self, id: i64) -> SqlResult<Option<Person>> {
        let mut stmt = self.conn.prepare("SELECT id, name, face_count, avatar_face_id, created_time, updated_time FROM persons WHERE id=?1")?;
        let mut rows = stmt.query(params![id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(Person {
                id: row.get(0)?,
                name: row.get(1)?,
                face_count: row.get(2)?,
                avatar_face_id: row.get(3)?,
                created_time: row.get(4)?,
                updated_time: row.get(5)?,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn list_persons(&self) -> SqlResult<Vec<Person>> {
        let mut stmt = self.conn.prepare("SELECT id, name, face_count, avatar_face_id, created_time, updated_time FROM persons ORDER BY name")?;
        let rows = stmt.query_map([], |row| Ok(Person {
            id: row.get(0)?,
            name: row.get(1)?,
            face_count: row.get(2)?,
            avatar_face_id: row.get(3)?,
            created_time: row.get(4)?,
            updated_time: row.get(5)?,
        }))?;
        rows.collect()
    }

    pub fn update_person_name(&self, id: i64, name: &str) -> SqlResult<()> {
        self.conn.execute("UPDATE persons SET name=?1, updated_time=?2 WHERE id=?3", params![name, chrono::Utc::now().timestamp(), id])?;
        Ok(())
    }

    pub fn update_person_face_count(&self, id: i64, count: i32) -> SqlResult<()> {
        self.conn.execute("UPDATE persons SET face_count=?1, updated_time=?2 WHERE id=?3", params![count, chrono::Utc::now().timestamp(), id])?;
        Ok(())
    }

    pub fn set_person_avatar(&self, person_id: i64, face_id: i64) -> SqlResult<()> {
        self.conn.execute("UPDATE persons SET avatar_face_id=?1, updated_time=?2 WHERE id=?3", params![face_id, chrono::Utc::now().timestamp(), person_id])?;
        Ok(())
    }

    // Library folders
    pub fn add_folder(&self, path: &Path, name: &str) -> SqlResult<i64> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT OR REPLACE INTO library_folders (path, name, created_time) VALUES (?1, ?2, ?3)",
            params![path.to_string_lossy().to_string(), name, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_folders(&self) -> SqlResult<Vec<(i64, String, String, i32)>> {
        let mut stmt = self.conn.prepare("SELECT id, path, name, image_count FROM library_folders")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?;
        rows.collect()
    }

    // Metadata cache
    pub fn set_cache(&self, key: &str, value: &str) -> SqlResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO metadata_cache (key, value, updated_time) VALUES (?1, ?2, ?3)",
            params![key, value, chrono::Utc::now().timestamp()],
        )?;
        Ok(())
    }

    pub fn get_cache(&self, key: &str) -> SqlResult<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM metadata_cache WHERE key=?1")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }
}