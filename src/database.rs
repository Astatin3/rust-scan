use std::{net::IpAddr, sync::Arc, time::Instant};

use rocksdb::{Cache, ColumnFamily, DB, IteratorMode, Options, WriteBatch};
use serde::{Deserialize, Serialize};

use crate::{port_scan::port_scan::PortScanResult, service_scan::service_scan::ServiceScanResult};

// Global settings for optimal performance
const BLOCK_CACHE_SIZE_MB: usize = 512; // 512MB block cache
const WRITE_BUFFER_SIZE_MB: usize = 64; // 64MB write buffer
const NUM_PARALLEL_THREADS: usize = 8; // Number of threads for parallel operations
const BATCH_SIZE: usize = 1000; // Batch size for writes

pub struct ResultDatabase {
    pub path: String,
    options: Options,
    columns: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatabaseResult {
    pub id: String,       // Row identifier
    pub ports: Vec<i32>,  // Array of string values
    pub services: String, // json services
}

impl DatabaseResult {
    pub fn to_string(&self) -> String {
        let mut str = "".to_string();

        str += format!(
            "{} - ports: [{}] services: [{}]",
            self.id,
            join_nums(&self.ports, ","),
            &self.services
        )
        .as_str();

        str
    }
    pub fn encode(&self, buf: &mut Vec<u8>) {
        let values = vec![self.ports_to_string(), self.services.clone()];

        // Write number of values
        buf.extend_from_slice(&(values.len() as u32).to_le_bytes());

        // Write each value
        for value in values {
            let value_bytes = value.as_bytes();
            buf.extend_from_slice(&(value_bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(value_bytes);
        }
    }
    // Binary decoding of row data
    pub fn decode(key: &str, data: &[u8]) -> Option<Self> {
        println!("{}", data.len());
        if data.len() < 8 {
            return None;
        }

        let mut pos = 0;

        if pos + 4 > data.len() {
            return None;
        }
        let mut values_count_bytes = [0u8; 4];
        values_count_bytes.copy_from_slice(&data[pos..pos + 4]);
        let values_count = u32::from_le_bytes(values_count_bytes) as usize;
        pos += 4;

        // Read values
        let mut values = Vec::with_capacity(values_count);
        for _ in 0..values_count {
            if pos + 4 > data.len() {
                println!("error1!");
                return None;
            }

            let mut value_len_bytes = [0u8; 4];
            value_len_bytes.copy_from_slice(&data[pos..pos + 4]);
            let value_len = u32::from_le_bytes(value_len_bytes) as usize;
            pos += 4;

            if pos + value_len > data.len() {
                println!("error!");
                return None;
            }

            let value = String::from_utf8_lossy(&data[pos..pos + value_len]).to_string();
            values.push(value);
            pos += value_len;
        }

        Some(DatabaseResult {
            id: key.to_string(),
            ports: if 1 > 0 {
                split_nums(values[0].as_str(), ",")
            } else {
                Vec::new()
            },
            services: if 1 > 1 {
                values[1].clone()
            } else {
                String::new()
            },
        })
    }
    pub fn ports_to_string(&self) -> String {
        return join_nums(&self.ports, ",");
    }
}

pub fn join_nums(nums: &Vec<i32>, sep: &str) -> String {
    // 1. Convert numbers to strings
    let str_nums: Vec<String> = nums
        .iter()
        .map(|n| n.to_string()) // map every integer to a string
        .collect(); // collect the strings into the vector

    // 2. Join the strings. There's already a function for this.
    str_nums.join(sep)
}

pub fn split_nums(str: &str, sep: &str) -> Vec<i32> {
    if str.is_empty() {
        return vec![];
    };

    return str
        .split(sep)
        .map(|n| {
            if let Ok(num) = n.parse::<i32>() {
                return num;
            } else {
                return 0;
            }
        })
        .collect();
}

impl ResultDatabase {
    pub fn new(path: &str) -> Self {
        let mut options = Options::default();

        options.create_if_missing(true);
        options.create_missing_column_families(true);
        options.increase_parallelism(NUM_PARALLEL_THREADS as i32); // Use multiple background threads
        options.set_max_background_jobs(4);
        options.set_write_buffer_size(WRITE_BUFFER_SIZE_MB * 1024 * 1024); // Larger write buffer
        options.set_max_write_buffer_number(3); // Allow more write buffers
        options.set_target_file_size_base(64 * 1024 * 1024); // 64MB per SST file
        options.set_level_zero_file_num_compaction_trigger(4); // Start compaction after 4 L0 files
        options.set_level_zero_slowdown_writes_trigger(16); // Start slowing down writes after 16 L0 files
        options.set_level_zero_stop_writes_trigger(24); // Stop writes after 24 L0 files
        options.set_max_bytes_for_level_base(512 * 1024 * 1024); // 512MB for base level
        options.set_disable_auto_compactions(false); // Enable auto compactions
        options.optimize_level_style_compaction(WRITE_BUFFER_SIZE_MB * 1024 * 1024);
        options.set_max_total_wal_size(256 * 1024 * 1024); // 256MB max for WAL files
        options.set_keep_log_file_num(5); // Keep 5 log files
        options.set_log_level(rocksdb::LogLevel::Warn); // Minimal logging

        // Set up block cache for improved read performance
        let mut block_opts = rocksdb::BlockBasedOptions::default();
        block_opts.set_block_cache(&Cache::new_lru_cache(BLOCK_CACHE_SIZE_MB * 1024 * 1024));
        block_opts.set_bloom_filter(10.0, false);
        block_opts.set_whole_key_filtering(true);
        block_opts.set_cache_index_and_filter_blocks(true);
        block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
        options.set_block_based_table_factory(&block_opts);

        // Define column families for different indexes

        let column_families = vec![
            "default".to_string(),
            "ports".to_string(),
            "services".to_string(),
        ];

        Self {
            path: path.to_string(),
            options,
            columns: column_families,
        }
    }

    pub fn add_ping_results(
        &self,
        results: &Vec<IpAddr>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut string_rows = Vec::with_capacity(results.len()); // Pre-allocate capacity

        for result in results {
            string_rows.push(DatabaseResult {
                id: result.to_string(),
                ports: vec![],
                services: String::new(),
            });
        }

        return self.save_rows(string_rows);
    }

    pub fn add_tcp_results(
        &self,
        results: &Vec<PortScanResult>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut string_rows = Vec::with_capacity(results.len()); // Pre-allocate capacity

        for result in results {
            string_rows.push(result.to_database());
        }

        return self.save_rows(string_rows);
    }

    pub fn add_service_results(
        &self,
        results: &Vec<ServiceScanResult>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut string_rows = Vec::with_capacity(results.len()); // Pre-allocate capacity

        for result in results {
            let e = result.to_database();
            print!("{}", e.services);
            string_rows.push(e);
        }

        return self.save_rows(string_rows);
    }

    pub fn save_rows(
        &self,
        string_rows: Vec<DatabaseResult>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let db = Arc::new(DB::open_cf(&self.options, &self.path, &self.columns)?);
        let cf_default = db.cf_handle(&self.columns[0]).unwrap();
        let cf_ports = db.cf_handle(&self.columns[1]).unwrap();
        let cf_services = db.cf_handle(&self.columns[2]).unwrap();

        let start = Instant::now();
        let length = string_rows.len();

        // Split the rows into chunks for parallel processing
        let chunks: Vec<Vec<DatabaseResult>> = string_rows
            .chunks(BATCH_SIZE)
            .map(|chunk| chunk.to_vec())
            .collect();

        // Process chunks in parallel
        let elapsed = {
            let db_ref = Arc::clone(&db);
            let cf_default_ref = cf_default;

            // Create batches in parallel but write them sequentially
            let batches: Vec<WriteBatch> = chunks
                .into_iter()
                .map(|chunk| {
                    let mut batch = WriteBatch::default();

                    for row in chunk {
                        batch.put_cf(cf_default_ref, row.id.as_bytes(), &vec![]);

                        // Ports
                        batch.put_cf(
                            cf_ports,
                            row.id.as_bytes(),
                            row.ports_to_string().as_bytes(),
                        );

                        // Services
                        batch.put_cf(cf_services, row.id.as_bytes(), row.services.as_bytes());
                    }

                    batch
                })
                .collect();

            // Write all batches to the database
            for batch in batches {
                db_ref.write(batch)?;
            }

            // Force a flush to ensure all data is persisted
            db_ref.flush()?;

            start.elapsed()
        };

        println!("Saved {} rows in {}ms", length, elapsed.as_millis());

        Ok(())
    }

    pub fn get_row_by_host(&self, row: &str) -> Option<DatabaseResult> {
        let db = DB::open_cf(&self.options, &self.path, &self.columns);
        if db.is_err() {
            return None;
        };
        let db = db.unwrap();

        let cfs = vec![
            db.cf_handle(&self.columns[0]).unwrap(),
            db.cf_handle(&self.columns[1]).unwrap(),
            db.cf_handle(&self.columns[2]).unwrap(),
        ];

        return self.fetch_row(&db, row, &cfs);
    }

    pub fn get_rows_by_port(&self, port: &str) -> Vec<DatabaseResult> {
        if let Ok(result) = self.search_substring_in_column(self.columns[1].as_str(), port) {
            return result;
        } else {
            return Vec::new();
        }
    }

    pub fn get_rows_by_service(&self, port: &str) -> Vec<DatabaseResult> {
        if let Ok(result) = self.search_substring_in_column(self.columns[2].as_str(), port) {
            return result;
        } else {
            return Vec::new();
        }
    }

    pub fn search_substring_in_column(
        &self,
        column: &str,
        substring: &str,
    ) -> Result<Vec<DatabaseResult>, rocksdb::Error> {
        let db = Arc::new(DB::open_cf(&self.options, &self.path, &self.columns)?);

        let cf = db.cf_handle(column).unwrap();
        let cfs = vec![
            db.cf_handle(&self.columns[0]).unwrap(),
            db.cf_handle(&self.columns[1]).unwrap(),
            db.cf_handle(&self.columns[2]).unwrap(),
        ];

        let mut matching_keys: Vec<DatabaseResult> = Vec::new();

        // Use RocksDB's iterator for efficient scanning
        let iter = db.iterator_cf(cf, IteratorMode::Start);

        // Iterate through all key-value pairs in the column family
        for item in iter {
            let (key_bytes, value_bytes) = item?;

            // Convert value to string (assumes UTF-8 encoding)
            if let Ok(value_str) = std::str::from_utf8(&value_bytes) {
                // Check if the value contains the substring
                if value_str.contains(substring) {
                    // Convert key to string and add to results
                    if let Ok(key_str) = std::str::from_utf8(&key_bytes) {
                        if let Some(row) = self.fetch_row(&db, key_str, &cfs) {
                            matching_keys.push(row);
                        }
                    }
                }
            }
        }

        Ok(matching_keys)
    }

    fn fetch_row(&self, db: &DB, row_id: &str, cfs: &Vec<&ColumnFamily>) -> Option<DatabaseResult> {
        match db.get_cf(&cfs[0], row_id.as_bytes()) {
            Ok(Some(_)) => Some(DatabaseResult {
                id: row_id.to_string(),
                ports: split_nums(&self.row_to_string(db, row_id, &cfs[1]), ","),
                services: self.row_to_string(db, row_id, &cfs[2]),
            }),
            _ => None,
        }
    }

    fn row_to_string(&self, db: &DB, row_id: &str, cf: &ColumnFamily) -> String {
        if let Ok(Some(data)) = &db.get_cf(cf, row_id) {
            String::from_utf8_lossy(data).to_string()
        } else {
            String::new()
        }
    }
}
