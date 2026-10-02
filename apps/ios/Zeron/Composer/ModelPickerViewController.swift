import UIKit

final class ModelPickerViewController: UIViewController, UITableViewDataSource, UITableViewDelegate {
    private enum Filter: Equatable {
        case all
        case favorites
        case harness(String)
    }

    private struct Section {
        let title: String
        let models: [ModelChoice]
    }

    private static let favoritesStorageKey = "modelFavorites"

    private let models: [ModelChoice]
    private let selected: (harness: String, model: String?)
    private let lockedHarness: String?
    private let onSelect: (ModelChoice) -> Void
    private var filter: Filter = .all
    private var favorites = Set(UserDefaults.standard.stringArray(forKey: "modelFavorites") ?? [])
    private var sections: [Section] = []
    private let titleLabel = UILabel()
    private let subtitleLabel = UILabel()
    private let closeButton = UIButton(type: .system)
    private let search = UITextField()
    private let chipScroll = UIScrollView()
    private let chipStack = UIStackView()
    private let tableView = UITableView(frame: .zero, style: .plain)
    private let contentStack = UIStackView()

    init(
        models: [ModelChoice],
        selected: (harness: String, model: String?),
        lockedHarness: String? = nil,
        onSelect: @escaping (ModelChoice) -> Void
    ) {
        self.models = models
        self.selected = selected
        self.lockedHarness = lockedHarness
        self.onSelect = onSelect
        super.init(nibName: nil, bundle: nil)
        modalPresentationStyle = .pageSheet
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.elevated

        titleLabel.text = "Model"
        titleLabel.textColor = Palette.text
        titleLabel.font = scaled(.sansSemibold, size: 17, style: .headline)
        titleLabel.adjustsFontForContentSizeCategory = true
        subtitleLabel.text = lockedHarness.map(harnessLabel) ?? "Pick a model for this session"
        subtitleLabel.textColor = Palette.secondary
        subtitleLabel.font = scaled(.sans, size: 13, style: .subheadline)
        subtitleLabel.adjustsFontForContentSizeCategory = true
        let titles = UIStackView(arrangedSubviews: [titleLabel, subtitleLabel])
        titles.axis = .vertical
        titles.alignment = .leading
        titles.spacing = 2

        var closeConfiguration = UIButton.Configuration.plain()
        closeConfiguration.image = UIImage(
            systemName: "xmark",
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .semibold)
        )
        closeConfiguration.baseForegroundColor = Palette.secondary
        closeConfiguration.background.backgroundColor = Palette.controlFill
        closeConfiguration.background.cornerRadius = 15
        closeConfiguration.contentInsets = .zero
        closeButton.configuration = closeConfiguration
        closeButton.accessibilityLabel = "Close model picker"
        closeButton.addAction(UIAction { [weak self] _ in self?.dismiss(animated: true) }, for: .touchUpInside)
        closeButton.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            closeButton.widthAnchor.constraint(equalToConstant: 30),
            closeButton.heightAnchor.constraint(equalToConstant: 30),
        ])

        let header = UIStackView(arrangedSubviews: [titles, UIView(), closeButton])
        header.axis = .horizontal
        header.alignment = .center
        header.spacing = 12

        configureSearch()
        configureChips()
        configureTable()

        contentStack.axis = .vertical
        contentStack.alignment = .fill
        contentStack.spacing = 12
        contentStack.translatesAutoresizingMaskIntoConstraints = false
        contentStack.addArrangedSubview(header)
        contentStack.addArrangedSubview(search)
        contentStack.addArrangedSubview(chipScroll)
        contentStack.addArrangedSubview(tableView)
        view.addSubview(contentStack)

        NSLayoutConstraint.activate([
            contentStack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            contentStack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            contentStack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 16),
            contentStack.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -10),
            search.heightAnchor.constraint(equalToConstant: 38),
            chipScroll.heightAnchor.constraint(equalToConstant: 34),
        ])

        renderChips()
        renderSections()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        scrollToSelectedModel()
    }

    func present(from anchor: UIView, in presenter: UIViewController) {
        modalPresentationStyle = .pageSheet
        if let sheet = sheetPresentationController {
            sheet.detents = [.medium(), .large()]
            sheet.prefersGrabberVisible = true
            sheet.preferredCornerRadius = 22
        }
        if UIDevice.current.userInterfaceIdiom == .pad, let presentation = popoverPresentationController {
            presentation.sourceView = anchor
            presentation.sourceRect = anchor.bounds
        }
        presenter.present(self, animated: true)
    }

    private func configureSearch() {
        search.borderStyle = .none
        search.font = scaled(.sans, size: 14, style: .body)
        search.textColor = Palette.text
        search.tintColor = Palette.accent
        search.placeholder = "Search models…"
        search.accessibilityLabel = "Search models"
        search.accessibilityIdentifier = "model-search"
        search.autocorrectionType = .no
        search.clearButtonMode = .whileEditing
        search.backgroundColor = Palette.controlFill
        search.layer.cornerRadius = 10
        let icon = UIImageView(image: UIImage(
            systemName: "magnifyingglass",
            withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .medium)
        ))
        icon.tintColor = Palette.tertiary
        icon.contentMode = .center
        icon.frame = CGRect(x: 0, y: 0, width: 36, height: 36)
        search.leftView = icon
        search.leftViewMode = .always
        search.addTarget(self, action: #selector(queryChanged), for: .editingChanged)
    }

    private func configureChips() {
        chipScroll.showsHorizontalScrollIndicator = false
        chipScroll.translatesAutoresizingMaskIntoConstraints = false
        chipStack.axis = .horizontal
        chipStack.alignment = .center
        chipStack.spacing = 8
        chipStack.translatesAutoresizingMaskIntoConstraints = false
        chipScroll.addSubview(chipStack)
        NSLayoutConstraint.activate([
            chipStack.leadingAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.leadingAnchor),
            chipStack.trailingAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.trailingAnchor),
            chipStack.topAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.topAnchor),
            chipStack.bottomAnchor.constraint(equalTo: chipScroll.contentLayoutGuide.bottomAnchor),
            chipStack.heightAnchor.constraint(equalTo: chipScroll.frameLayoutGuide.heightAnchor),
        ])
    }

    private func configureTable() {
        tableView.backgroundColor = .clear
        tableView.separatorStyle = .none
        tableView.sectionHeaderTopPadding = 6
        tableView.rowHeight = UITableView.automaticDimension
        tableView.estimatedRowHeight = 64
        tableView.contentInsetAdjustmentBehavior = .never
        tableView.register(ModelPickerCell.self, forCellReuseIdentifier: ModelPickerCell.reuseIdentifier)
        tableView.dataSource = self
        tableView.delegate = self
    }

    @objc private func queryChanged() {
        renderSections()
    }

    private var harnesses: [String] {
        Self.harnesses(in: models, lockedTo: lockedHarness)
    }

    private var visibleFavoritesExist: Bool {
        models.contains { model in
            (lockedHarness == nil || model.harness == lockedHarness) && isFavorite(model)
        }
    }

    private var selectedModelID: String? {
        selected.model ?? models.first(where: { $0.harness == selected.harness })?.id
    }

    private func renderChips() {
        chipStack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        chipStack.addArrangedSubview(filterChip(title: "All", image: nil, selection: .all))
        if visibleFavoritesExist {
            chipStack.addArrangedSubview(
                filterChip(title: "Favorites", image: UIImage(systemName: "star.fill"), selection: .favorites)
            )
        }
        if harnesses.count > 1 {
            for harness in harnesses {
                chipStack.addArrangedSubview(
                    filterChip(title: harnessLabel(harness), image: BrandMarks.image(for: harness, side: 15), selection: .harness(harness))
                )
            }
        }
        chipScroll.isHidden = chipStack.arrangedSubviews.count == 1
    }

    private func filterChip(title: String, image: UIImage?, selection: Filter) -> UIButton {
        var configuration = UIButton.Configuration.plain()
        configuration.title = title
        configuration.image = image
        configuration.imagePadding = 6
        configuration.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 12)
        let chipFont = scaled(.sansMedium, size: 13, style: .subheadline)
        configuration.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attributes in
            var attributes = attributes
            attributes.font = chipFont
            return attributes
        }
        configuration.preferredSymbolConfigurationForImage = UIImage.SymbolConfiguration(pointSize: 11, weight: .semibold)
        let isSelected = filter == selection
        configuration.baseForegroundColor = isSelected ? Palette.accent : Palette.secondary
        configuration.background.backgroundColor = isSelected ? Palette.accentSoft : Palette.controlFill
        configuration.background.cornerRadius = 17
        let button = UIButton(configuration: configuration)
        button.accessibilityLabel = title
        button.accessibilityTraits = .button
        button.addAction(UIAction { [weak self] _ in
            self?.filter = selection
            self?.renderChips()
            self?.renderSections()
        }, for: .touchUpInside)
        return button
    }

    private func renderSections() {
        let query = (search.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let visible = models.filter { lockedHarness == nil || $0.harness == lockedHarness }
        switch filter {
        case .all:
            if query.isEmpty {
                let favorites = visible.filter(isFavorite)
                sections = []
                if !favorites.isEmpty { sections.append(Section(title: "Favorites", models: favorites)) }
                sections.append(contentsOf: group(visible))
            } else {
                sections = group(matching(visible, query: query))
            }
        case .favorites:
            let favoriteModels = visible.filter(isFavorite)
            let matches = matching(favoriteModels, query: query)
            sections = query.isEmpty
                ? (matches.isEmpty ? [] : [Section(title: "Favorites", models: matches)])
                : group(matches)
        case let .harness(harness):
            let harnessModels = visible.filter { $0.harness == harness }
            let matches = matching(harnessModels, query: query)
            sections = query.isEmpty
                ? (matches.isEmpty ? [] : [Section(title: harnessLabel(harness), models: matches)])
                : group(matches)
        }
        tableView.reloadData()
        tableView.backgroundView = emptyState(query: query)
    }

    private func matching(_ models: [ModelChoice], query: String) -> [ModelChoice] {
        guard !query.isEmpty else { return models }
        return models.filter { model in
            [model.label, model.id, model.description ?? "", model.harnessLabel]
                .contains { $0.localizedCaseInsensitiveContains(query) }
        }
    }

    private func emptyState(query: String) -> UILabel? {
        guard sections.isEmpty else { return nil }
        let label = UILabel()
        if !query.isEmpty {
            label.text = "No models match “\(query)”"
        } else if filter == .favorites {
            label.text = "Swipe a model left to add it to Favorites"
        } else {
            label.text = "No models available"
        }
        label.font = scaled(.sans, size: 14, style: .body)
        label.textColor = Palette.secondary
        label.textAlignment = .center
        label.numberOfLines = 0
        return label
    }

    private func scrollToSelectedModel() {
        guard let selectedModelID else { return }
        for (sectionIndex, section) in sections.enumerated() {
            guard let rowIndex = section.models.firstIndex(where: {
                $0.harness == selected.harness && $0.id == selectedModelID
            }) else { continue }
            tableView.scrollToRow(
                at: IndexPath(row: rowIndex, section: sectionIndex),
                at: .middle,
                animated: false
            )
            return
        }
    }

    private func toggleFavorite(_ model: ModelChoice) {
        let key = Self.favoriteKey(model)
        if favorites.contains(key) {
            favorites.remove(key)
        } else {
            favorites.insert(key)
        }
        UserDefaults.standard.set(favorites.sorted(), forKey: Self.favoritesStorageKey)
        renderChips()
        renderSections()
    }

    private func isFavorite(_ model: ModelChoice) -> Bool {
        favorites.contains(Self.favoriteKey(model))
    }

    private func group(_ models: [ModelChoice]) -> [Section] {
        var order: [String] = []
        var byHarness: [String: [ModelChoice]] = [:]
        for model in models {
            if byHarness[model.harness] == nil { order.append(model.harness) }
            byHarness[model.harness, default: []].append(model)
        }
        return order.compactMap { harness in
            byHarness[harness].map { Section(title: harnessLabel(harness), models: $0) }
        }
    }

    private func harnessLabel(_ harness: String) -> String {
        models.first(where: { $0.harness == harness })?.harnessLabel ?? HarnessNames.label(harness)
    }

    private func scaled(_ font: FaceRole, size: CGFloat, style: UIFont.TextStyle) -> UIFont {
        UIFontMetrics(forTextStyle: style).scaledFont(for: Fonts.ui(font, size))
    }

    private static func harnesses(in models: [ModelChoice], lockedTo harness: String?) -> [String] {
        var seen = Set<String>()
        return models.compactMap { model in
            guard (harness == nil || model.harness == harness), seen.insert(model.harness).inserted else { return nil }
            return model.harness
        }
    }

    private static func favoriteKey(_ model: ModelChoice) -> String {
        "\(model.harness)/\(model.id)"
    }

    func numberOfSections(in tableView: UITableView) -> Int {
        sections.count
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        sections[section].models.count
    }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let model = sections[indexPath.section].models[indexPath.row]
        let cell = tableView.dequeueReusableCell(
            withIdentifier: ModelPickerCell.reuseIdentifier,
            for: indexPath
        ) as! ModelPickerCell
        let selectedModel = model.harness == selected.harness && model.id == selectedModelID
        cell.configure(model: model, selected: selectedModel, favorite: isFavorite(model)) { [weak self] in
            self?.toggleFavorite(model)
        }
        return cell
    }

    func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        let model = sections[indexPath.section].models[indexPath.row]
        UISelectionFeedbackGenerator().selectionChanged()
        onSelect(model)
        dismiss(animated: true)
    }

    func tableView(_ tableView: UITableView, viewForHeaderInSection section: Int) -> UIView? {
        guard !shouldHideLockedHarnessHeader(in: section) else { return nil }
        let container = UIView()
        let label = UILabel()
        label.text = sections[section].title.uppercased()
        label.textColor = Palette.tertiary
        label.font = scaled(.sansMedium, size: 11, style: .caption1)
        label.adjustsFontForContentSizeCategory = true
        label.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 20),
            label.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -20),
            label.topAnchor.constraint(equalTo: container.topAnchor, constant: 8),
            label.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -2),
        ])
        return container
    }

    func tableView(_ tableView: UITableView, heightForHeaderInSection section: Int) -> CGFloat {
        shouldHideLockedHarnessHeader(in: section) ? 0 : 30
    }

    private func shouldHideLockedHarnessHeader(in section: Int) -> Bool {
        guard
            lockedHarness != nil,
            filter == .all,
            sections.count == 1,
            (search.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else {
            return false
        }
        return sections[section].title != "Favorites"
    }

    func tableView(_ tableView: UITableView, trailingSwipeActionsConfigurationForRowAt indexPath: IndexPath) -> UISwipeActionsConfiguration? {
        let model = sections[indexPath.section].models[indexPath.row]
        let favorited = isFavorite(model)
        let action = UIContextualAction(
            style: .normal,
            title: favorited ? "Unfavorite" : "Favorite"
        ) { [weak self] _, _, completion in
            self?.toggleFavorite(model)
            completion(true)
        }
        action.image = UIImage(systemName: favorited ? "star.slash" : "star.fill")
        action.backgroundColor = favorited ? Palette.tertiary : Palette.accent
        let configuration = UISwipeActionsConfiguration(actions: [action])
        configuration.performsFirstActionWithFullSwipe = false
        return configuration
    }

    func tableView(
        _ tableView: UITableView,
        contextMenuConfigurationForRowAt indexPath: IndexPath,
        point: CGPoint
    ) -> UIContextMenuConfiguration? {
        let model = sections[indexPath.section].models[indexPath.row]
        let favorited = isFavorite(model)
        let favorite = UIAction(
            title: favorited ? "Unfavorite" : "Favorite",
            image: UIImage(systemName: favorited ? "star.slash" : "star.fill")
        ) { [weak self] _ in
            self?.toggleFavorite(model)
        }
        let use = UIAction(title: "Use this model") { [weak self] _ in
            guard let self else { return }
            UISelectionFeedbackGenerator().selectionChanged()
            self.onSelect(model)
            self.dismiss(animated: true)
        }
        return UIContextMenuConfiguration(identifier: model.id as NSString, previewProvider: nil) { _ in
            UIMenu(children: [favorite, use])
        }
    }
}

private final class ModelPickerCell: UITableViewCell {
    static let reuseIdentifier = "ModelPickerCell"

    private let selectionBackground = UIView()
    private let mark = UIImageView()
    private let title = UILabel()
    private let detail = UILabel()
    private let favoriteMark = UIImageView(image: UIImage(systemName: "star.fill"))
    private let checkmark = UIImageView(image: UIImage(systemName: "checkmark"))
    private let labels = UIStackView()
    private let rowContent = UIStackView()
    private var selectedModel = false
    private var toggleFavorite: (() -> Void)?

    override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
        super.init(style: style, reuseIdentifier: reuseIdentifier)
        backgroundColor = .clear
        contentView.backgroundColor = .clear
        selectionBackground.layer.cornerRadius = 12
        selectionBackground.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(selectionBackground)
        NSLayoutConstraint.activate([
            selectionBackground.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 10),
            selectionBackground.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -10),
            selectionBackground.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 3),
            selectionBackground.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -3),
            contentView.heightAnchor.constraint(greaterThanOrEqualToConstant: 56),
        ])

        mark.contentMode = .scaleAspectFit
        mark.setContentHuggingPriority(.required, for: .horizontal)
        title.font = dynamicFont(.sansMedium, size: 15, style: .body)
        title.textColor = Palette.text
        title.lineBreakMode = .byTruncatingTail
        title.adjustsFontForContentSizeCategory = true
        detail.font = dynamicFont(.sans, size: 12.5, style: .caption1)
        detail.textColor = Palette.secondary
        detail.numberOfLines = 2
        detail.lineBreakMode = .byTruncatingTail
        detail.adjustsFontForContentSizeCategory = true
        labels.axis = .vertical
        labels.alignment = .fill
        labels.spacing = 2
        labels.addArrangedSubview(title)
        labels.addArrangedSubview(detail)
        labels.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)

        favoriteMark.tintColor = Palette.accent
        favoriteMark.contentMode = .scaleAspectFit
        favoriteMark.setContentHuggingPriority(.required, for: .horizontal)
        checkmark.tintColor = Palette.accent
        checkmark.contentMode = .scaleAspectFit
        checkmark.setContentHuggingPriority(.required, for: .horizontal)
        rowContent.axis = .horizontal
        rowContent.alignment = .center
        rowContent.spacing = 9
        rowContent.translatesAutoresizingMaskIntoConstraints = false
        rowContent.addArrangedSubview(mark)
        rowContent.addArrangedSubview(labels)
        rowContent.addArrangedSubview(favoriteMark)
        rowContent.addArrangedSubview(checkmark)
        contentView.addSubview(rowContent)
        NSLayoutConstraint.activate([
            rowContent.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 20),
            rowContent.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -20),
            rowContent.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 8),
            rowContent.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -8),
            mark.widthAnchor.constraint(equalToConstant: 20),
            mark.heightAnchor.constraint(equalToConstant: 20),
            favoriteMark.widthAnchor.constraint(equalToConstant: 11),
            favoriteMark.heightAnchor.constraint(equalToConstant: 11),
            checkmark.widthAnchor.constraint(equalToConstant: 14),
            checkmark.heightAnchor.constraint(equalToConstant: 14),
        ])
        selectionStyle = .default
        isAccessibilityElement = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func setHighlighted(_ highlighted: Bool, animated: Bool) {
        super.setHighlighted(highlighted, animated: animated)
        updateSelectionBackground()
    }

    func configure(model: ModelChoice, selected: Bool, favorite: Bool, onToggleFavorite: @escaping () -> Void) {
        mark.image = BrandMarks.image(for: model.harness, side: 20)
        mark.isHidden = mark.image == nil
        title.text = model.label
        title.textColor = selected ? Palette.accent : Palette.text
        title.font = dynamicFont(selected ? .sansSemibold : .sansMedium, size: 15, style: .body)
        detail.text = model.description
        detail.isHidden = model.description?.isEmpty ?? true
        favoriteMark.isHidden = !favorite
        checkmark.isHidden = !selected
        selectedModel = selected
        toggleFavorite = onToggleFavorite
        accessibilityIdentifier = "model-row-\(model.id)"
        accessibilityLabel = [model.label, model.description]
            .compactMap { $0 }
            .filter { !$0.isEmpty }
            .joined(separator: ", ")
        accessibilityValue = selected ? "selected" : "not selected"
        accessibilityTraits = .button
        let actionName = favorite ? "Unfavorite" : "Favorite"
        accessibilityCustomActions = [
            UIAccessibilityCustomAction(name: actionName) { [weak self] _ in
                guard let self, let toggleFavorite = self.toggleFavorite else { return false }
                toggleFavorite()
                return true
            },
        ]
        updateSelectionBackground()
    }

    private func updateSelectionBackground() {
        selectionBackground.backgroundColor = isHighlighted
            ? Palette.controlFill
            : (selectedModel ? Palette.accentSoft : .clear)
    }

    private func dynamicFont(_ face: FaceRole, size: CGFloat, style: UIFont.TextStyle) -> UIFont {
        UIFontMetrics(forTextStyle: style).scaledFont(for: Fonts.ui(face, size))
    }
}
