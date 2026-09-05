{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.ananicy-rs;
  configFile = pkgs.writeText "ananicy.conf" (lib.generators.toKeyValue { } cfg.settings);
  extraRules = pkgs.writeText "extraRules" (
    lib.concatMapStringsSep "\n" (l: builtins.toJSON l) cfg.extraRules
  );
  extraTypes = pkgs.writeText "extraTypes" (
    lib.concatMapStringsSep "\n" (l: builtins.toJSON l) cfg.extraTypes
  );
  extraCgroups = pkgs.writeText "extraCgroups" (
    lib.concatMapStringsSep "\n" (l: builtins.toJSON l) cfg.extraCgroups
  );
in
{
  options.services.ananicy-rs = {
    enable = lib.mkEnableOption "Ananicy-Rs, an auto nice daemon rewrite in Rust";

    package = lib.mkOption {
      type = lib.types.package;
      description = "The ananicy-rs package to use.";
    };

    rulesProvider = lib.mkPackageOption pkgs "ananicy-cpp" { example = "ananicy-cpp"; } // {
      description = ''
        Which package to copy default rules, types, cgroups from.
        By default, we use ananicy-cpp rules since ananicy-rs does not ship its own.
      '';
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "--systemd" ];
      description = "Additional command-line arguments passed to the ananicy-rs daemon.";
    };

    settings = lib.mkOption {
      type =
        with lib.types;
        attrsOf (oneOf [
          int
          bool
          str
        ]);
      default = { };
      example = {
        apply_nice = false;
      };
      description = ''
        See <https://github.com/Nefelim4ag/Ananicy/blob/master/ananicy.d/ananicy.conf>
      '';
    };

    extraRules = lib.mkOption {
      type = with lib.types; listOf attrs;
      default = [ ];
      description = ''
        Rules to write in 'nixRules.rules'.
      '';
      example = [
        {
          name = "eog";
          type = "Image-Viewer";
        }
      ];
    };

    extraTypes = lib.mkOption {
      type = with lib.types; listOf attrs;
      default = [ ];
      description = ''
        Types to write in 'nixTypes.types'.
      '';
      example = [
        {
          type = "my_type";
          nice = 19;
        }
      ];
    };

    extraCgroups = lib.mkOption {
      type = with lib.types; listOf attrs;
      default = [ ];
      description = ''
        Cgroups to write in 'nixCgroups.cgroups'.
      '';
      example = [
        {
          cgroup = "cpu80";
          CPUQuota = 80;
        }
      ];
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];

    environment.etc."ananicy.d".source =
      pkgs.runCommand "ananicyfiles"
        {
          preferLocalBuild = true;
        }
        ''
          mkdir -p $out
          # Copy rules from rulesProvider
          if [[ -d "${cfg.rulesProvider}/etc/ananicy.d/00-default" ]]; then
            cp -r ${cfg.rulesProvider}/etc/ananicy.d/* $out
          else
            cp -r ${cfg.rulesProvider}/etc/ananicy.d/* $out || cp -r ${cfg.rulesProvider}/* $out
          fi

          # configured through .settings
          rm -f $out/ananicy.conf
          cp ${configFile} $out/ananicy.conf
          ${lib.optionalString (cfg.extraRules != [ ]) "cp ${extraRules} $out/nixRules.rules"}
          ${lib.optionalString (cfg.extraTypes != [ ]) "cp ${extraTypes} $out/nixTypes.types"}
          ${lib.optionalString (cfg.extraCgroups != [ ]) "cp ${extraCgroups} $out/nixCgroups.cgroups"}
        '';

    services.ananicy-rs.settings =
      let
        mkOD = lib.mkOptionDefault;
      in
      {
        cgroup_load = mkOD true;
        type_load = mkOD true;
        rule_load = mkOD true;
        apply_nice = mkOD true;
        apply_ioclass = mkOD true;
        apply_ionice = mkOD true;
        apply_sched = mkOD true;
        apply_oom_score_adj = mkOD true;
        apply_cgroup = mkOD true;
        loglevel = mkOD "warn";
        log_applied_rule = mkOD false;
      };

    systemd = {
      packages = [ cfg.package ];

      services."ananicy-rs" = {
        serviceConfig.ExecStart = lib.mkForce [
          "" # Clear the existing default
          "${cfg.package}/bin/ananicy-rs ${lib.escapeShellArgs cfg.extraArgs} start"
        ];

        wantedBy = [ "multi-user.target" ];
      };
    };
  };
}
