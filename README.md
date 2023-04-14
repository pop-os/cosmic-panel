# Cosmic Panel (WIP)

### Building and Installing .deb

`dpkg-buildpackage -b -d`  
`cd ..`  
`sudo dpkg -i cosmic-panel_0.1.0_amd64.deb`  

### Configuring the panel / dock  
See the provided configs for the panel and dock in `data/default_schema`. 
The `com.system76.CosmicPanel` directory contains a key called entries, which is a list of profiles to be loaded. 
Each profile then has its own directory, for example, `com.system76.CosmicPanel.Panel`. 
You can make changes to the keys in this directory to alter the config. 
After making changes to copies of the provided config in data `data/`, you may install each to `$HOME/.config/cosmic/`
`find data/default_schema_copy -type f -exec install -Dm0644 {} {{$HOME/.config/cosmic}}/{} \;`

### Usage  
cosmic-panel

### Installing Plugins and Applets  
See the following for examples of applets and plugins which can be installed and used:  
https://github.com/pop-os/cosmic-applets  
 
