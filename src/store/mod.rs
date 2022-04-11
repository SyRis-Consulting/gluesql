mod alter_table;
mod store;
mod store_mut;

use std::sync::{Mutex, MutexGuard};

pub use alter_table::*;
mod auto_increment;

pub use auto_increment::AutoIncrement;

use {
	crate::{Result},
	serde::{Deserialize, Serialize},
	std::fmt::Debug,
	thiserror::Error,
};

pub use {store::Store, store_mut::StoreMut};

#[derive(Error, Serialize, Debug, PartialEq)]
pub enum StorageError {
	#[error("this storage has not yet implemented this method")]
	Unimplemented,
	#[error("tried to connect to an unknown storage")]
	UnknownConnection,
}

#[derive(Serialize, Deserialize)]
pub enum Connection {
	Unknown,
	#[cfg(feature = "sled-storage")]
	Sled(String),
	#[cfg(feature = "csv-storage")]
	CSV(String, crate::CSVSettings),
	#[cfg(feature = "sheet-storage")]
	Sheet(String),
}
impl Default for Connection {
	fn default() -> Self {
		Connection::Unknown
	}
}
impl TryFrom<Connection> for Storage {
	type Error = crate::Error;
	fn try_from(connection: Connection) -> Result<Storage> {
		use {
			crate::{CSVStorage, SheetStorage, SledStorage},
			Connection::*,
		};
		let storage: Mutex<Box<dyn FullStorage>> = Mutex::new(match &connection {
			#[cfg(feature = "sled-storage")]
			Sled(path) => Box::new(SledStorage::new(path)?),
			#[cfg(feature = "csv-storage")]
			CSV(path, settings) => Box::new(CSVStorage::new_with_settings(path, settings.clone())?),
			#[cfg(feature = "sheet-storage")]
			Sheet(path) => Box::new(SheetStorage::new(path)?),
			Unknown => return Err(StorageError::UnknownConnection.into()),
		});
		Ok(Storage {
			storage,
			source_connection: connection,
		})
	}
}

pub struct Storage {
	source_connection: Connection,
	storage: Mutex<Box<dyn FullStorage>>,
}
impl Storage {
	pub fn new(storage: Box<dyn FullStorage>) -> Self {
		let storage = Mutex::new(storage);
		Self {
			storage,
			source_connection: Connection::default(),
		}
	}
	/*pub fn replace(&mut self, storage: Box<dyn FullStorage>) {
		self.storage.replace(storage);
	}
	pub fn take(&mut self) -> Box<dyn FullStorage> {
		self.storage
			.take()
			.expect("Unreachable: Storage wasn't replaced!")
	}*/
	pub fn get(&self) -> MutexGuard<Box<dyn FullStorage>> {
		self.storage
			.lock()
			.expect("Unreachable: Storage wasn't replaced!")
	}
	pub fn get_mut(&mut self) -> &mut Box<dyn FullStorage> {
		self.storage
			.get_mut()
			.expect("Unreachable: Storage wasn't replaced!")
	}
	pub fn into_source(self) -> Connection {
		self.source_connection
	}
	pub fn from_source(connection: Connection) -> Result<Self> {
		connection.try_into()
	}
}

pub type StorageInner = dyn FullStorage;

pub trait FullStorage: Store + StoreMut + AlterTable + AutoIncrement {}
