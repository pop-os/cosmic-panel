# Cosmic Panel (WIP)

## Usage

### Building and Installing .deb

`dpkg-buildpackage -b -d`  
`cd ..`  
`sudo dpkg -i cosmic-panel_0.1.0_amd64.deb`  

### Configuring the panel / dock  
See the provided config.ron for an example configuration for a panel and dock. It can be placed in `~/.config/cosmic-panel/config.ron` or any xdg config directory for cosmic-panel

### Usage  
cosmic-panel "[name of config profile]"

### Installing Plugins and Applets  
See the following for examples of applets and plugins which can be installed and used:  
https://github.com/pop-os/cosmic-applets  
https://github.com/wash2/cosmic-app-button-plugin  
https://github.com/wash2/dock_plugin_apps  
 