Rusty bandwidth is a completly rewrtitten version of the bandwidth hero proxy server in rust.

This version uses way less system resources when running (in itself just 15M of ram) and should be easier to selfhost due to only needing to run a single executable.

By default it uses the port 8080 and 512MB of ram for caching purposes. Both can be changed with launch parameters.

Has webp encoding only, Avif was tried but found out to be too slow

Hardware encoding is not yet supported
