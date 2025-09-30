#!/bin/bash

# Option 1: Simple trap-based wrapper
# Save as: run_pipeline.sh

#!/bin/bash
set -e

# Store the PID of the main process
MAIN_PID=""

# Cleanup function
cleanup() {
    echo "Caught signal, cleaning up..."
    
    if [ ! -z "$MAIN_PID" ]; then
        # First try SIGTERM (graceful)
        echo "Sending SIGTERM to process group..."
        kill -TERM -$MAIN_PID 2>/dev/null || true
        
        # Give it 2 seconds to clean up
        sleep 2
        
        # Then force kill if still running
        if kill -0 $MAIN_PID 2>/dev/null; then
            echo "Process still running, sending SIGKILL..."
            kill -KILL -$MAIN_PID 2>/dev/null || true
        fi
    fi
    
    # Kill any remaining child processes
    pkill -P $$ 2>/dev/null || true
    
    exit 0
}

# Set up signal handlers
trap cleanup SIGINT SIGTERM

# Run the Rust program in a new process group
# The 'setsid' makes it a session leader so we can kill the whole group
echo "Starting pipeline..."
setsid cargo run --release --bin uart_io_add_one "$@" &
MAIN_PID=$!

# Wait for the process
wait $MAIN_PID

