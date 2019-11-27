# Step 0

* In the file `/Cargo.toml`, add `step_0` to `workspace.members`.

* In the directory `/step_0/`, run the following command:
  
      cargo init --lib

* In the file `/step_0/Cargo.toml`, change `package.name` to `step_0_wasm`.
  
* In the file `/step_0/Cargo.toml`, add the following lines:

      [lib]
      crate_type = ["cdylib"]

* In the directory `/step_0/static`, create the following new files:

  * `wasm.js`