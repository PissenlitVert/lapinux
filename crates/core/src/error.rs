//! Erreurs communes.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("E/S : {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON : {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML : {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("TOML : {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("base de données : {0}")]
    Db(#[from] redb::Error),
    #[error("base de données : {0}")]
    DbTxn(String),
    #[error("daemon injoignable : {0}")]
    Ipc(String),
    /// Le socket a répondu, mais pas à temps : le daemon est vivant et
    /// occupé, ce qui n'est pas une panne et ne doit pas le faire relancer.
    #[error("délai IPC dépassé : {0}")]
    IpcTimeout(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<redb::DatabaseError> for Error {
    fn from(e: redb::DatabaseError) -> Self {
        Error::DbTxn(e.to_string())
    }
}

impl From<redb::TransactionError> for Error {
    fn from(e: redb::TransactionError) -> Self {
        Error::DbTxn(e.to_string())
    }
}

impl From<redb::TableError> for Error {
    fn from(e: redb::TableError) -> Self {
        Error::DbTxn(e.to_string())
    }
}

impl From<redb::StorageError> for Error {
    fn from(e: redb::StorageError) -> Self {
        Error::DbTxn(e.to_string())
    }
}

impl From<redb::CommitError> for Error {
    fn from(e: redb::CommitError) -> Self {
        Error::DbTxn(e.to_string())
    }
}
