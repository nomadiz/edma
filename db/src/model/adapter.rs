use async_trait::async_trait;

use crate::{err::Error, util::get_absolute_path, SimpleTransaction};
use std::{pin::Pin, sync::Arc};

#[derive(Debug, Clone)]
pub enum StorageVariant {
	RelationalStore,
	KeyValueStore,
}

#[derive(Debug, Clone)]
pub enum StorageAdapterName {
	RocksDB,
	ReDB,
	EchoDB,
}

#[derive(Debug, Clone)]
pub struct StorageAdapter<T> {
	pub name: StorageAdapterName,
	pub path: String,
	pub db_instance: Pin<Arc<T>>,
	pub variant: StorageVariant,
}

impl<T> StorageAdapter<T> {
	pub fn new(
		name: StorageAdapterName,
		path: String,
		db_instance: T,
		variant: StorageVariant,
	) -> Result<Self, Error> {
		Ok(StorageAdapter {
			name,
			path: get_absolute_path(&path),
			db_instance: Arc::pin(db_instance),
			variant,
		})
	}
}

#[async_trait]
pub trait DatastoreAdapter {
	type Transaction: SimpleTransaction;
	// # Create new database transaction
	// Set `rw` default to false means readable but not readable
	async fn transaction(&self, rw: bool) -> Result<Self::Transaction, Error>;

	fn default() -> Self
	where
		Self: Sized,
	{
		todo!();
	}

	fn spawn(&self) -> Self
	where
		Self: Sized,
	{
		todo!()
	}

	fn path(&self) -> &str;
}
