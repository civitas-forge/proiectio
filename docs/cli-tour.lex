The CLI

    Proiectio can be used both as a rust library or a full CLI tool, with feature party.

    Projecting files (writing):
        $ proiectio write /tmp/myconfig.toml  <path>...# projects /tmp/myconfig.toml and paths into cwd
        $ proiectio  write /tmp/my-tree --tree # projects  tmp/my-tree into cwd, my-tree must be a directory
        $ proiectio write /tmp/my-mapping.yaml # projects the resolved tree from mapping , including content into cwd.
    Status (list):
        $ proiectio list <path>... # List the files in the specified paths
    Remove (rm):
        $ proiectio rm <path>... # Remove previously projected  files in the specified paths, if any such has been modified, exits non 0.
    :: shell ::


    Additionally, options:

    --dry-run # Run the command without making any changes
    --chmod <value> # Set the permissions of the files to the values specified
