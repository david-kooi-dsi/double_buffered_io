use std::env;
use std::io;
use mio_serial::{SerialPortBuilder, SerialStream, FlowControl, DataBits, Parity, StopBits};

 use crate::io::repeat;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <port_name> <baud_rate>", args[0]);
        eprintln!("Example: {} /dev/ttyUSB0 115200", args[0]);
        std::process::exit(1);
    }

    let port_name = &args[1];
    let baud_rate: u32 = args[2].parse()
        .map_err(|e| format!("Invalid baud rate: {}", e))?;

    println!("Testing mio_serial dual stream opening on port: {} at {} baud", 
             port_name, baud_rate);
    println!("{}", "=" .repeat(60));

    // Create the serial port builder configuration

    let builder = mio_serial::new(port_name, baud_rate)
        .path(port_name)
        .baud_rate(baud_rate)
        .flow_control(FlowControl::None)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One);
 // Test 1: Try to open first stream
 println!("\n[Test 1] Opening first stream...");
 let mut read_stream = match SerialStream::open(&builder) {
     Ok(stream) => {
         println!("✓ Successfully opened first stream");
         stream
     }
     Err(e) => {
         println!("✗ Failed to open first stream: {}", e);
         return Err(Box::new(e));
     }
 };

 // Check exclusivity of first stream
 let exclusive_status = read_stream.exclusive();
 println!("  First stream exclusivity status: {}", exclusive_status);

 println!("  changing exclusivity to false");
    match read_stream.set_exclusive(false) {
        Ok(()) => {
            println!("Successfully changed exclusivity");
        },
        Err(e) => {
            println!("Could not change exclusivity: {e}");
        }
    }

 // Test 2: Try to open second stream on the same port
 println!("\n[Test 2] Attempting to open second stream on the same port...");
 match SerialStream::open(&builder) {
     Ok(mut write_stream) => {
         println!("✓ Successfully opened second stream (unexpected if port is exclusive)");
         println!("  Second stream exclusivity status: {}", write_stream.exclusive());
         
         // If we got here, both streams are open
         println!("\n[Info] Both streams successfully opened on the same port!");
         println!("  This indicates the port is NOT exclusive by default.");
     }
     Err(e) => {
         println!("✗ Failed to open second stream: {}", e);
         println!("  This is expected if the port is exclusive.");
     }
 }

 // Test 3: Test the pair() method
 println!("\n[Test 3] Testing SerialStream::pair() method...");
 match SerialStream::pair() {
     Ok((stream1, stream2)) => {
         println!("✓ Successfully created a pair of streams");
         println!("  Stream 1 exclusivity: {}", stream1.exclusive());
         println!("  Stream 2 exclusivity: {}", stream2.exclusive());
         println!("  Note: pair() creates a virtual serial port pair, not connected to {}", port_name);
     }
     Err(e) => {
         println!("✗ Failed to create stream pair: {}", e);
     }
 }

 // Test 4: Try setting exclusivity
 println!("\n[Test 4] Testing exclusivity control...");
 println!("  Setting first stream to exclusive mode...");
 match read_stream.set_exclusive(true) {
     Ok(()) => {
         println!("✓ Successfully set exclusive mode");
         println!("  Exclusivity after setting: {}", read_stream.exclusive());
         
         // Now try to open another stream
         println!("  Attempting to open another stream with exclusive mode set...");
         match SerialStream::open(&builder) {
             Ok(_) => {
                 println!("✓ Unexpectedly opened another stream despite exclusive mode");
             }
             Err(e) => {
                 println!("✗ Failed to open another stream (expected): {}", e);
             }
         }
     }
     Err(e) => {
         println!("✗ Failed to set exclusive mode: {}", e);
         println!("  This might not be supported on this platform.");
     }
 }

 // Test 5: Reset exclusivity and try again
 println!("\n[Test 5] Testing non-exclusive mode...");
 println!("  Setting first stream to non-exclusive mode...");
 match read_stream.set_exclusive(false) {
     Ok(()) => {
         println!("✓ Successfully set non-exclusive mode");
         println!("  Exclusivity after unsetting: {}", read_stream.exclusive());
         
         println!("  Attempting to open another stream with non-exclusive mode...");
         match SerialStream::open(&builder) {
             Ok(_) => {
                 println!("✓ Successfully opened another stream in non-exclusive mode");
             }
             Err(e) => {
                 println!("✗ Failed to open another stream: {}", e);
             }
         }
     }
     Err(e) => {
         println!("✗ Failed to set non-exclusive mode: {}", e);
     }
 }

 println!("\n{}", "=".repeat(60));
 println!("Test Summary:");
 println!("- Port tested: {}", port_name);
 println!("- Baud rate: {}", baud_rate);
 println!("- Default exclusivity: {}", exclusive_status);
 println!("\nNote: Results may vary based on OS and serial port driver.");
 
 Ok(())
}