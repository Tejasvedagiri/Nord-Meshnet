# Synology (DSM 7.2+, Container Manager)

1. **Container Manager → Project → Create**, source: *Create docker-compose.yml*.
   Use a project (not the container wizard), which can't set `sysctls`.
2. Store the token in a `.env` file in the project folder, or use the secrets
   example.
3. `/dev/net/tun`: present on DSM 7.2 on most models. If `docker logs`
   shows a TUN error, load the module once over SSH and add it to a boot task:

   ```sh
   sudo insmod /lib/modules/tun.ko
   ```

4. For macvlan, the parent interface is usually `eth0` or `ovs_eth0` (when
   Open vSwitch is enabled for Virtual Machine Manager).
