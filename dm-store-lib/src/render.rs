//! Tiny output helpers shared by dm-store-cli and dm-manager-cli so their
//! Set/Add/Del/Instances flows don't drift apart line-by-line.

use crate::AddResult;

/// Print an instance-number list for a multi-instance object.
pub fn print_instances(table_path: &str, nums: &[u32]) {
    if nums.is_empty() {
        println!("No instances for {}", table_path);
    } else {
        for n in nums {
            println!("{}", n);
        }
    }
}

/// Print the result of a successful Add.
pub fn print_add_result(result: &AddResult) {
    println!(
        "Added instance {} at {}",
        result.instance_number, result.path
    );
}

/// Print confirmation of a successful Delete.
pub fn print_deleted(path: &str) {
    println!("Deleted {}", path);
}
