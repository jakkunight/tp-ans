{
  pkgs,
  # lib,
  config,
  # inputs,
  ...
}:

{
  # https://devenv.sh/basics/
  # env.GREET = "devenv";
  # dotenv = {
  #   enable = "";
  # };
  env = {
    # SOPS_AGE_KEY = "~/.config/sops/age/dev-key.txt";
    # SOPS_AGE_KEY_FILE = "~/.config/sops/age/dev-key.txt";
  };

  # https://devenv.sh/packages/
  packages = with pkgs; [
    git
    bacon
    cargo-nextest
    sqlx-cli
    sops
    secretspec
    age
  ];

  # https://devenv.sh/languages/
  # languages.rust.enable = true;
  languages = {
    rust = {
      enable = true;
      channel = "stable";
    };
    javascript = {
      enable = true;
      bun = {
        enable = true;
      };
    };
    typescript = {
      enable = true;
    };
  };

  # https://devenv.sh/processes/
  # processes.dev.exec = "${lib.getExe pkgs.watchexec} -n -- ls -la";
  process.manager.implementation = "process-compose";

  processes = {
    # dev-lab = {
    #   cwd = "${config.devenv.root}/lab";
    #   exec = ''
    #     export SOPS_AGE_KEY=~/.config/sops/age/dev-key.txt; secretspec run -- export DATABASE_URL="${
    #       config.secretspec.secrets.LAB_DB_URL or ""
    #     }" && cargo run
    #   '';
    # };
    # dev-farm = {
    #   cwd = "${config.devenv.root}/farm";
    #   exec = ''
    #     export SOPS_AGE_KEY=~/.config/sops/age/dev-key.txt; secretspec run -- export DATABASE_URL="${
    #       config.secretspec.secrets.FARM_DB_URL or ""
    #     }" && cargo run
    #   '';
    # };
  };

  # https://devenv.sh/services/
  # services.postgres.enable = true;
  services = {
    postgres = {
      enable = true;
      initialDatabases = [
        {
          name = config.secretspec.secrets.LAB_DB_DATABASE or "lab";
          schema = /home/jakku/Documents/COLUMBIA/ans/sistema-tickets/lab/database/schema.sql;
          user = config.secretspec.secrets.LAB_DB_USER or "lab";
          pass = config.secretspec.secrets.LAB_DB_PASS or "lab";
        }
        # {
        #   name = config.secretspec.secrets.FARM_DB_DATABASE or "";
        #   schema = null;
        #   user = config.secretspec.secrets.FARM_DB_USER or "";
        #   pass = config.secretspec.secrets.FARM_DB_PASS or "";
        # }
      ];
      listen_addresses = "127.0.0.1";
      port = 5432;
    };
  };

  # https://devenv.sh/scripts/
  # scripts.hello.exec = ''
  #   echo hello from $GREET
  # '';
  # scripts = {
  #   run-lab.exec = ''
  #     export SOPS_AGE_KEY_FILE="~/.config/sops/age/dev-key.txt";
  #     exec secretspec run -- zsh -c '
  #       cd "$DEVENV_ROOT/lab"
  #       export DATABASE_URL="$LAB_DATABASE_URL"
  #       exec cargo run
  #     '
  #   '';
  #   run-farm.exec = ''
  #     export SOPS_AGE_KEY_FILE="~/.config/sops/age/dev-key.txt";
  #     exec secretspec run -- zsh -c '
  #       cd "$DEVENV_ROOT/farm"
  #       export DATABASE_URL="$FARM_DATABASE_URL"
  #       exec cargo run
  #     '
  #   '';
  #   wrap-lab-db.exec = ''
  #     export SOPS_AGE_KEY_FILE="~/.config/sops/age/dev-key.txt";
  #     exec secretspec run -- zsh -c '
  #       cd "$DEVENV_ROOT/lab"
  #       export DATABASE_URL="$LAB_DATABASE_URL"
  #       exec "$@"
  #     '
  #     -- "$@"
  #   '';
  #   wrap-farm-db.exec = ''
  #     export SOPS_AGE_KEY_FILE="~/.config/sops/age/dev-key.txt";
  #     exec secretspec run -- zsh -c '
  #       cd "$DEVENV_ROOT/farm"
  #       export DATABASE_URL="$FARM_DATABASE_URL"
  #       exec "$@"
  #     '
  #     -- "$@"
  #   '';
  # };

  # https://devenv.sh/basics/
  # enterShell = ''
  #   hello         # Run scripts directly
  #   git --version # Use packages
  # '';

  # https://devenv.sh/tasks/
  # tasks = {
  #   "myproj:setup".exec = "mytool build";
  #   "devenv:enterShell".after = [ "myproj:setup" ];
  # };

  # https://devenv.sh/tests/
  # enterTest = ''
  #   echo "Running tests"
  #   git --version | grep --color=auto "${pkgs.git.version}"
  # '';

  # https://devenv.sh/git-hooks/
  # git-hooks.hooks.shellcheck.enable = true;

  # See full reference at https://devenv.sh/reference/options/
}
