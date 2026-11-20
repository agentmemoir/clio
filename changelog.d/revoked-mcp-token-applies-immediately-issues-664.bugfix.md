A revoked MCP HTTP bearer token now stops working on a running server. The server re-reads the token store on every request, so `clio auth token revoke` takes effect without a restart.
